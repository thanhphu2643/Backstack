import AppKit
import Foundation
import Observation

func formatBytes<T: BinaryInteger>(_ n: T) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(n), countStyle: .file)
}

struct Tenant: Identifiable, Hashable {
    let name: String
    /// Newest first.
    let backups: [Backup]
    var id: String { name }
    var size: UInt64 { backups.reduce(0) { $0 + $1.size } }
}

@MainActor @Observable
final class Store {
    var folders: [String] { didSet { save("folders", folders) } }
    var buckets: [BucketConfig] { didSet { saveCodable("buckets", buckets) } }
    var targets: [TargetConfig] { didSet { saveCodable("targets", targets) } }
    var targetID: UUID { didSet { save("target", targetID.uuidString) } }
    /// Name of restored databases, with {tenant}, {date} (yyyymmdd) and {time} (hhmm),
    /// e.g. "myapp_{tenant}_{date}".
    var dbTemplate: String { didSet { save("dbTemplate", dbTemplate) } }
    static let defaultTemplate = "{tenant}_{date}"
    /// Backups kept on this Mac per tenant when cleaning up.
    var keep: Int { didSet { save("keep", keep) } }

    /// No target was ever set up: find the containers before talking to one.
    private var needsTargets: Bool
    var backups: [Backup] = []
    var scanErrors: [String] = []
    var databases: [Database] = []
    var databaseError: String?
    var scanning = false
    var showSettings = false
    var error: String?
    var notice: String?

    var selectedTenant: String?
    var selectedBackup: Backup.ID?
    var details: [String: Details] = [:]
    var detailErrors: [String: String] = [:]
    var inspecting: Set<String> = []

    private(set) var jobID: UInt64?
    var job: JobStatus?

    init() {
        let d = UserDefaults.standard
        folders = d.stringArray(forKey: "folders") ?? [FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads").path]
        buckets = Self.load("buckets") ?? []
        // The first version kept a single container and user. Without either, the running
        // PostgreSQL containers are looked up on the first scan.
        let saved: [TargetConfig]? = Self.load("targets") ?? d.string(forKey: "container").map {
            [TargetConfig(container: $0, user: d.string(forKey: "user") ?? "postgres")]
        }
        needsTargets = saved == nil
        let targets = saved ?? [TargetConfig()]
        self.targets = targets
        targetID = d.string(forKey: "target").flatMap(UUID.init).flatMap { id in targets.first { $0.id == id }?.id } ?? targets[0].id
        keep = d.object(forKey: "keep") as? Int ?? 2
        dbTemplate = d.string(forKey: "dbTemplate") ?? Self.defaultTemplate
    }

    private func save(_ key: String, _ value: Any) { UserDefaults.standard.set(value, forKey: key) }
    private func saveCodable<T: Encodable>(_ key: String, _ value: T) {
        UserDefaults.standard.set(try? JSONEncoder().encode(value), forKey: key)
    }
    private static func load<T: Decodable>(_ key: String) -> T? {
        UserDefaults.standard.data(forKey: key).flatMap { try? JSONDecoder().decode(T.self, from: $0) }
    }

    var target: TargetConfig { targets.first { $0.id == targetID } ?? targets[0] }
    private var s3: [S3Access] { buckets.map(S3Access.init) }

    var tenants: [Tenant] {
        Dictionary(grouping: backups, by: \.tenant)
            .map { Tenant(name: $0.key, backups: $0.value.sorted { ($0.takenAt ?? 0) > ($1.takenAt ?? 0) }) }
            .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    var currentTenant: Tenant? { tenants.first { $0.name == selectedTenant } }
    var currentBackup: Backup? { backups.first { $0.id == selectedBackup } }
    func backup(_ id: Backup.ID) -> Backup? { backups.first { $0.id == id } }

    /// Older backups on this Mac beyond the newest `keep` of each tenant. S3 has its own retention.
    var pruneCandidates: [Backup] { tenants.flatMap { $0.backups.filter(\.hasLocal).dropFirst(keep) } }
    var pruneSize: UInt64 { pruneCandidates.flatMap(\.localCopies).reduce(0) { $0 + $1.size } }

    // MARK: loading

    func refresh() async {
        guard !scanning else { return }
        scanning = true
        error = nil
        let (folders, s3) = (folders, s3)
        do {
            let result = try await Task.detached(priority: .userInitiated) { try Core.scan(folders: folders, buckets: s3) }.value
            backups = result.backups
            scanErrors = result.errors
            if let t = selectedTenant, !backups.contains(where: { $0.tenant == t }) { selectedTenant = nil }
            if let b = selectedBackup, !backups.contains(where: { $0.id == b }) { selectedBackup = nil }
            if selectedTenant == nil { selectedTenant = tenants.first?.name }
        } catch {
            self.error = error.localizedDescription
        }
        scanning = false
        if needsTargets { await detectTargets() }
        await loadDatabases()
    }

    /// One target per running PostgreSQL container; the first publishes a port if any does.
    private func detectTargets() async {
        guard let found = try? await Task.detached(operation: { try Core.containers() }).value, !found.isEmpty else { return }
        needsTargets = false
        targets = found.map { TargetConfig(container: $0.name) }
        targetID = targets[0].id
    }

    func loadDatabases() async {
        let access = PGAccess(target)
        do {
            databases = try await Task.detached { try Core.databases(access) }.value
            databaseError = nil
        } catch {
            databases = []
            databaseError = error.localizedDescription
        }
    }

    func selectTarget(_ id: UUID) {
        targetID = id
        databases = []
        Task { await loadDatabases() }
    }

    /// Reads every row count. Copies only in S3 are downloaded for it, so callers ask first.
    func inspect(_ b: Backup) async {
        guard details[b.id] == nil, !inspecting.contains(b.id) else { return }
        inspecting.insert(b.id)
        defer { inspecting.remove(b.id) }
        let (copy, s3) = (b.primary, s3)
        do {
            details[b.id] = try await Task.detached(priority: .userInitiated) { try Core.inspect(copy, buckets: s3) }.value
            detailErrors[b.id] = nil
        } catch {
            detailErrors[b.id] = error.localizedDescription
        }
    }

    // MARK: restore

    /// The name template filled in for one backup, lowercased to a name PostgreSQL takes
    /// unquoted.
    func defaultDatabaseName(_ b: Backup) -> String {
        Self.databaseName(template: dbTemplate, tenant: b.tenant, date: b.date)
    }

    nonisolated static func databaseName(template: String, tenant: String, date: Date?) -> String {
        let clean = { (s: String) in String(s.lowercased().map { $0.isASCII && ($0.isLetter || $0.isNumber) ? $0 : "_" }) }
        let template = template.contains("{tenant}") ? template : template + "{tenant}"
        // "nwc_{tenant}" for a tenant already called "nwc_x" gives "nwc_x", not "nwc_nwc_x".
        let lead = clean(String(template[..<template.range(of: "{tenant}")!.lowerBound]))
        var tenant = clean(tenant)
        if !lead.isEmpty, tenant.hasPrefix(lead) { tenant.removeFirst(lead.count) }
        let format = { (pattern: String) -> String in
            guard let date else { return "restore" }
            let f = DateFormatter()
            f.dateFormat = pattern
            f.timeZone = .current
            return f.string(from: date)
        }
        var name = clean(template
            .replacingOccurrences(of: "{tenant}", with: tenant)
            .replacingOccurrences(of: "{date}", with: format("yyyyMMdd"))
            .replacingOccurrences(of: "{time}", with: format("HHmm")))
        if name.first?.isNumber ?? true { name = "_" + name }
        return String(name.prefix(63))
    }

    func databaseExists(_ name: String) -> Bool { databases.contains { $0.name == name } }

    var restoring: Bool { jobID != nil && job?.finished != true }

    func restore(_ b: Backup, into database: String, replace: Bool) async {
        let (access, copy, s3) = (PGAccess(target), b.primary, s3)
        job = JobStatus(phase: "start", bytes: 0, total: 0, finished: false, error: nil)
        do {
            let id = try await Task.detached {
                try Core.restore(access, copy: copy, buckets: s3, database: database, replace: replace)
            }.value
            jobID = id
            while true {
                let s = try await Task.detached { try Core.job(id) }.value
                job = s
                if s.finished { break }
                try await Task.sleep(for: .milliseconds(300))
            }
        } catch {
            job = JobStatus(phase: "failed", bytes: 0, total: 0, finished: true, error: error.localizedDescription)
        }
        await loadDatabases()
    }

    func cancelRestore() {
        if let id = jobID { Core.cancel(id) }
    }

    func resetJob() {
        jobID = nil
        job = nil
    }

    // MARK: cleanup

    func drop(_ db: Database) async {
        let access = PGAccess(target)
        do {
            try await Task.detached { try Core.drop(access, database: db.name) }.value
            notice = t("Dropped database \(db.name)", "Đã xoá database \(db.name)")
        } catch {
            self.error = error.localizedDescription
        }
        await loadDatabases()
    }

    /// Moves the local copies of the older backups to the Trash.
    func prune() async {
        let paths = pruneCandidates.flatMap { $0.localCopies.map(\.path) }
        let size = pruneSize
        let failed = await Task.detached {
            paths.filter { (try? FileManager.default.trashItem(at: URL(fileURLWithPath: $0), resultingItemURL: nil)) == nil }.count
        }.value
        notice = t("Moved \(formatBytes(size)) to the Trash", "Đã chuyển \(formatBytes(size)) vào Thùng rác")
            + (failed > 0 ? t(" · \(failed) failed", " · \(failed) mục lỗi") : "")
        await refresh()
    }

    func reveal(_ path: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    // MARK: settings

    func checkBucket(_ c: BucketConfig) async -> Result<Int, Error> {
        let a = S3Access(c)
        return await Task.detached { Result { try Core.listBucket(a) } }.value
    }

    func checkTarget(_ c: TargetConfig) async -> Result<Int, Error> {
        let a = PGAccess(c)
        return await Task.detached { Result { try Core.databases(a).count } }.value
    }

    func removeBucket(_ c: BucketConfig) {
        buckets.removeAll { $0.id == c.id }
        Keychain.set(c.id, "")
    }

    func removeTarget(_ c: TargetConfig) {
        guard targets.count > 1 else { return }
        targets.removeAll { $0.id == c.id }
        Keychain.set(c.id, "")
        if targetID == c.id { selectTarget(targets[0].id) }
    }
}
