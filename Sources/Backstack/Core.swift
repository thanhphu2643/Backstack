import CBackstackCore
import Foundation
import Security

// Mirrors the JSON types of core/src.

struct BackupCopy: Decodable, Hashable {
    enum Kind: String, Codable { case folder, archive, dump }
    let path: String
    let kind: Kind
    let size: UInt64
    let remote: Bool

    var kindLabel: String {
        switch kind {
        case .folder: t("Folder", "Thư mục")
        case .archive: remote ? "S3" : ".tar.gz"
        case .dump: remote ? "S3 .dump" : ".dump"
        }
    }
}

struct Backup: Decodable, Identifiable, Hashable {
    let id: String
    let tenant: String
    let company: String?
    let takenAt: UInt64?
    let appVersion: String?
    let includes: [String]
    let dbName: String?
    let legacy: Bool
    let copies: [BackupCopy]

    var date: Date? { takenAt.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    var size: UInt64 { copies.reduce(0) { $0 + $1.size } }
    /// The copy restores read from: local before S3, and a folder needs no unpacking.
    var primary: BackupCopy { copies[0] }
    var localCopies: [BackupCopy] { copies.filter { !$0.remote } }
    var hasLocal: Bool { !primary.remote }
    var inS3: Bool { copies.contains(where: \.remote) }
}

struct ScanResult: Decodable {
    let backups: [Backup]
    let errors: [String]
}

struct TableRows: Decodable, Hashable {
    let name: String
    let rows: UInt64
}

struct Details: Decodable {
    let pgVersion: String?
    let sqlBytes: UInt64
    let tables: [TableRows]
    let totalRows: UInt64
    let files: UInt64
    let filesBytes: UInt64
}

struct Database: Decodable, Identifiable, Hashable {
    let name: String
    let size: UInt64
    var id: String { name }
}

struct JobStatus: Decodable {
    let phase: String
    let bytes: UInt64
    let total: UInt64
    let finished: Bool
    let error: String?
    var fraction: Double { total > 0 ? min(1, Double(bytes) / Double(total)) : 0 }
}

struct PGContainer: Decodable, Hashable {
    let name: String
    let image: String
    let published: Bool
}

// MARK: settings

/// An S3-compatible bucket holding backups as `{prefix}{tenant}/{file}`.
/// The secret key lives in the Keychain.
struct BucketConfig: Codable, Identifiable, Hashable, Sendable {
    var id = UUID()
    var endpoint = ""
    var port = 443
    var useSSL = true
    var region = ""
    var bucket = ""
    var prefix = ""
    var accessKey = ""

    var label: String { "s3://\(bucket)/\(prefix.trimmingCharacters(in: CharacterSet(charactersIn: "/")))" }

    func json(secret: String) -> [String: Any] {
        ["endpoint": endpoint, "port": port, "use_ssl": useSSL, "region": region, "bucket": bucket,
         "prefix": prefix, "access_key": accessKey, "secret_key": secret]
    }
}

/// Where restores go. The password lives in the Keychain.
struct TargetConfig: Codable, Identifiable, Hashable, Sendable {
    enum Via: String, Codable, CaseIterable { case docker, server }
    var id = UUID()
    var name = ""
    var via = Via.docker
    var container = "postgres"
    var host = "localhost"
    var port = 5432
    var user = "postgres"
    var sslmode = "prefer"
    /// Servers refuse drops and overwrites unless this is on.
    var allowDrop = false

    var label: String {
        if !name.isEmpty { return name }
        return via == .docker ? container : "\(user)@\(host):\(port)"
    }

    var detail: String { via == .docker ? "Docker · \(container)" : "\(user)@\(host):\(port)" }
    var canDrop: Bool { via == .docker || allowDrop }

    func json(password: String) -> [String: Any] {
        switch via {
        case .docker: ["via": "docker", "container": container, "user": user]
        case .server: ["via": "server", "host": host, "port": port, "user": user, "password": password,
                       "sslmode": sslmode, "allow_drop": allowDrop]
        }
    }
}

enum Keychain {
    private static let service = "com.nguyenthanhphu.Backstack"

    static func get(_ id: UUID) -> String {
        let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                kSecAttrAccount as String: id.uuidString, kSecReturnData as String: true]
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return "" }
        return String(decoding: data, as: UTF8.self)
    }

    static func set(_ id: UUID, _ value: String) {
        let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service,
                                kSecAttrAccount as String: id.uuidString]
        SecItemDelete(q as CFDictionary)
        guard !value.isEmpty else { return }
        var add = q
        add[kSecValueData as String] = Data(value.utf8)
        SecItemAdd(add as CFDictionary, nil)
    }
}

/// A bucket or target with its secret, ready to hand to the core.
struct S3Access: @unchecked Sendable {
    let json: [String: Any]
    let bucket: String
    init(_ c: BucketConfig) {
        json = c.json(secret: Keychain.get(c.id))
        bucket = c.bucket
    }
}

struct PGAccess: @unchecked Sendable {
    let json: [String: Any]
    init(_ c: TargetConfig) { json = c.json(password: Keychain.get(c.id)) }
}

// MARK: calls

private struct Started: Decodable { let job: UInt64 }
private struct Done: Decodable { let ok: Bool }
private struct Listed: Decodable { let objects: Int }

enum Core {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    static func scan(folders: [String], buckets: [S3Access]) throws -> ScanResult {
        try call(backstack_scan, ["folders": folders, "buckets": buckets.map(\.json)])
    }

    /// Number of objects in the bucket, to check the settings.
    static func listBucket(_ s3: S3Access) throws -> Int {
        let l: Listed = try call(backstack_list_bucket, s3.json)
        return l.objects
    }

    private static func source(_ copy: BackupCopy, _ buckets: [S3Access]) throws -> [String: Any] {
        guard copy.remote else { return ["path": copy.path] }
        guard let b = buckets.first(where: { copy.path.hasPrefix("s3://\($0.bucket)/") }) else {
            throw Failure(message: t("No S3 settings left for \(copy.path)", "Không còn cấu hình S3 cho \(copy.path)"))
        }
        return ["path": copy.path, "s3": b.json]
    }

    static func inspect(_ copy: BackupCopy, buckets: [S3Access]) throws -> Details {
        try call(backstack_inspect, ["source": source(copy, buckets), "kind": copy.kind.rawValue])
    }

    /// Running PostgreSQL containers, those publishing a port first.
    static func containers() throws -> [PGContainer] {
        try call(backstack_containers, [:])
    }

    static func databases(_ t: PGAccess) throws -> [Database] {
        try call(backstack_databases, t.json)
    }

    static func drop(_ t: PGAccess, database: String) throws {
        let _: Done = try call(backstack_drop, ["target": t.json, "database": database])
    }

    static func restore(_ t: PGAccess, copy: BackupCopy, buckets: [S3Access], database: String, replace: Bool) throws -> UInt64 {
        let started: Started = try call(backstack_restore, [
            "target": t.json, "source": source(copy, buckets), "kind": copy.kind.rawValue,
            "database": database, "replace": replace,
        ])
        return started.job
    }

    static func job(_ id: UInt64) throws -> JobStatus {
        try call(backstack_job, ["job": id])
    }

    static func cancel(_ id: UInt64) {
        let _: Done? = try? call(backstack_cancel, ["job": id])
    }

    private static func call<T: Decodable>(
        _ fn: (UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?, _ input: [String: Any]
    ) throws -> T {
        let json = String(decoding: try JSONSerialization.data(withJSONObject: input), as: UTF8.self)
        guard let out = json.withCString({ fn($0) }) else { throw Failure(message: "The Rust core returned nothing") }
        defer { backstack_free(out) }
        let data = Data(String(cString: out).utf8)
        if let err = try? JSONDecoder().decode([String: String].self, from: data), let msg = err["error"] {
            throw Failure(message: msg)
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(T.self, from: data)
    }
}

// MARK: comparing two backups

struct TableChange: Identifiable, Hashable {
    let name: String
    let before: UInt64?
    let after: UInt64?
    var id: String { name }
    var delta: Int64 { Int64(after ?? 0) - Int64(before ?? 0) }
    /// Had rows, now has none or is gone: the kind of change worth stopping on.
    var emptied: Bool { (before ?? 0) > 0 && (after ?? 0) == 0 }
}

struct Comparison {
    let changes: [TableChange]
    let unchanged: Int
    let rowsBefore: UInt64
    let rowsAfter: UInt64

    /// Changed tables, emptied ones first, then by how much they moved.
    init(old: Details, new: Details) {
        let a = Dictionary(old.tables.map { ($0.name, $0.rows) }, uniquingKeysWith: +)
        let b = Dictionary(new.tables.map { ($0.name, $0.rows) }, uniquingKeysWith: +)
        let all = Set(a.keys).union(b.keys).map { TableChange(name: $0, before: a[$0], after: b[$0]) }
        changes = all.filter { $0.before != $0.after }.sorted {
            if $0.emptied != $1.emptied { return $0.emptied }
            if abs($0.delta) != abs($1.delta) { return abs($0.delta) > abs($1.delta) }
            return $0.name < $1.name
        }
        unchanged = all.count - changes.count
        rowsBefore = old.totalRows
        rowsAfter = new.totalRows
    }
}
