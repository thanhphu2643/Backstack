import AppKit
import SwiftUI

struct ContentView: View {
    @Environment(Store.self) private var store
    @State private var showDatabases = false
    @State private var confirmPrune = false

    var body: some View {
        @Bindable var store = store
        NavigationSplitView {
            List(store.tenants, selection: $store.selectedTenant) { TenantRow(tenant: $0).tag($0.name) }
                .navigationSplitViewColumnWidth(min: 200, ideal: 240)
                .overlay {
                    if store.tenants.isEmpty && !store.scanning {
                        ContentUnavailableView(t("No backups yet", "Chưa có bản sao lưu"), systemImage: "archivebox",
                                               description: Text(t("Add a folder or an S3 bucket in the settings.",
                                                                   "Thêm thư mục hoặc bucket S3 trong Cài đặt.")))
                    }
                }
        } content: {
            if let tenant = store.currentTenant {
                List(tenant.backups, selection: $store.selectedBackup) { BackupRow(backup: $0).tag($0.id) }
                    .navigationTitle(tenant.name)
                    .navigationSplitViewColumnWidth(min: 260, ideal: 320)
            } else {
                ContentUnavailableView(t("Pick a tenant", "Chọn một tenant"), systemImage: "building.2")
            }
        } detail: {
            if let backup = store.currentBackup {
                BackupDetail(backup: backup)
            } else {
                ContentUnavailableView(t("Pick a backup", "Chọn một bản sao lưu"), systemImage: "clock.arrow.circlepath")
            }
        }
        .onChange(of: store.selectedTenant) { store.selectedBackup = store.currentTenant?.backups.first?.id }
        .toolbar {
            ToolbarItem {
                if store.scanning {
                    ProgressView().controlSize(.small)
                } else {
                    Button { Task { await store.refresh() } } label: { Label(t("Rescan", "Quét lại"), systemImage: "arrow.clockwise") }
                        .help(t("Rescan", "Quét lại"))
                }
            }
            // A menu picker keeps the chosen target's name visible in the toolbar.
            ToolbarItem {
                Picker(t("Restore into", "Khôi phục vào"), selection: .init(get: { store.targetID }, set: { store.selectTarget($0) })) {
                    ForEach(store.targets) { Text($0.label).tag($0.id) }
                }
                .pickerStyle(.menu)
                .help(t("PostgreSQL to restore into", "Postgres để khôi phục vào"))
            }
            ToolbarItem {
                Button { showDatabases = true } label: { Label(t("Databases", "Database"), systemImage: "cylinder.split.1x2") }
                    .help(t("Databases in \(store.target.label)", "Database trong \(store.target.label)"))
            }
            ToolbarItem {
                Button { confirmPrune = true } label: { Label(t("Clean up old backups", "Dọn bản cũ"), systemImage: "trash") }
                    .help(t("Clean up old backups", "Dọn bản cũ"))
                    .disabled(store.pruneCandidates.isEmpty)
            }
            ToolbarItem {
                Button { store.showSettings = true } label: { Label(t("Settings", "Cài đặt"), systemImage: "gearshape") }
                    .help(t("Settings", "Cài đặt"))
            }
        }
        .safeAreaInset(edge: .bottom) { StatusBar() }
        .sheet(isPresented: $showDatabases) { DatabasesView() }
        .sheet(isPresented: $store.showSettings) { SettingsView() }
        .confirmationDialog(pruneTitle, isPresented: $confirmPrune) {
            Button(t("Move to Trash", "Chuyển vào Thùng rác"), role: .destructive) { Task { await store.prune() } }
        } message: {
            Text(store.pruneCandidates.map { "\($0.tenant) · \(dateText($0.date))" }.joined(separator: "\n"))
        }
        .task { await store.refresh() }
        .frame(minWidth: 1000, minHeight: 600)
    }

    private var pruneTitle: String {
        let (keep, n, size) = (store.keep, store.pruneCandidates.count, formatBytes(store.pruneSize))
        return t("Keep the newest \(keep) backups of each tenant on this Mac and remove \(n) older ones (\(size))? Copies in S3 are not touched.",
                 "Giữ \(keep) bản mới nhất mỗi tenant trên máy, bỏ \(n) bản cũ (\(size))? Bản trên S3 không bị đụng tới.")
    }
}

struct StatusBar: View {
    @Environment(Store.self) private var store

    private var problem: String? {
        store.error ?? store.scanErrors.first.map { "S3 \($0)" } ?? store.databaseError.map { "Postgres \(store.target.label): \($0)" }
    }

    var body: some View {
        if store.notice != nil || problem != nil {
            HStack {
                if let e = problem {
                    Label(e, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red)
                } else if let n = store.notice {
                    Label(n, systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                }
                Spacer()
                Button(t("Dismiss", "Đóng")) {
                    store.notice = nil
                    store.error = nil
                    store.scanErrors = []
                }
                .buttonStyle(.borderless)
            }
            .font(.callout)
            .lineLimit(2)
            .textSelection(.enabled)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.bar)
        }
    }
}

struct TenantRow: View {
    let tenant: Tenant

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(tenant.name).fontWeight(.medium)
            HStack(spacing: 4) {
                let n = tenant.backups.count
                Text(t("\(n) \(n == 1 ? "backup" : "backups") · \(formatBytes(tenant.size))", "\(n) bản · \(formatBytes(tenant.size))"))
                if let d = tenant.backups.first?.date {
                    Text("·")
                    Text(d, format: .relative(presentation: .named))
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }
}

struct BackupRow: View {
    @Environment(Store.self) private var store
    let backup: Backup

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 3) {
                Text(dateText(backup.date)).fontWeight(.medium)
                HStack(spacing: 4) {
                    ForEach(backup.copies, id: \.path) { Badge(text: $0.kindLabel, tint: $0.remote ? .blue : .secondary) }
                    if backup.legacy { Badge(text: t("no metadata", "không metadata"), tint: .orange) }
                    if store.databaseExists(store.defaultDatabaseName(backup)) {
                        Badge(text: t("restored", "đã khôi phục"), tint: .green)
                    }
                }
            }
            Spacer()
            Text(formatBytes(backup.primary.size)).monospacedDigit().foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }
}

struct Badge: View {
    let text: String
    var tint: Color = .secondary

    var body: some View {
        Text(text)
            .font(.caption2.weight(.medium))
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .foregroundStyle(tint)
            .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 4))
    }
}

struct BackupDetail: View {
    @Environment(Store.self) private var store
    let backup: Backup
    @State private var restoring = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(backup.company ?? backup.tenant).font(.title2.bold())
                    Text("\(backup.tenant) · \(dateText(backup.date))").foregroundStyle(.secondary)
                }
                Button { restoring = true } label: {
                    Label(t("Restore into \(store.target.label)…", "Khôi phục vào \(store.target.label)…"),
                          systemImage: "arrow.down.to.line")
                }
                .controlSize(.large)
                .buttonStyle(.borderedProminent)
                .disabled(store.restoring)

                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
                    fact(t("App version", "Phiên bản app"), backup.appVersion)
                    fact(t("Source database", "Database gốc"), backup.dbName)
                    fact(t("Includes", "Gồm"), backup.includes.isEmpty ? nil : backup.includes.joined(separator: ", "))
                    if let d = store.details[backup.id] {
                        fact("PostgreSQL", d.pgVersion)
                        fact("database.sql", formatBytes(d.sqlBytes))
                        if d.files > 0 {
                            fact(t("Files", "Tệp"), t("\(d.files) files · \(formatBytes(d.filesBytes))", "\(d.files) tệp · \(formatBytes(d.filesBytes))"))
                        }
                    }
                }

                VStack(alignment: .leading, spacing: 6) {
                    Text(t("Copies", "Bản lưu")).font(.headline)
                    ForEach(backup.copies, id: \.path) { copy in
                        HStack {
                            Badge(text: copy.kindLabel, tint: copy.remote ? .blue : .secondary)
                            Text(copy.path).font(.caption).lineLimit(1).truncationMode(.middle).foregroundStyle(.secondary)
                                .textSelection(.enabled)
                            Spacer()
                            Text(formatBytes(copy.size)).font(.caption).monospacedDigit()
                            if !copy.remote {
                                Button { store.reveal(copy.path) } label: { Image(systemName: "magnifyingglass") }
                                    .buttonStyle(.borderless)
                                    .help(t("Show in Finder", "Hiện trong Finder"))
                            }
                        }
                    }
                }

                tables
                CompareSection(backup: backup)
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .task(id: backup.id) { if backup.hasLocal { await store.inspect(backup) } }
        .sheet(isPresented: $restoring) { RestoreSheet(backup: backup) }
    }

    @ViewBuilder private func fact(_ label: String, _ value: String?) -> some View {
        if let value {
            GridRow {
                Text(label).foregroundStyle(.secondary)
                Text(value).textSelection(.enabled)
            }
        }
    }

    @ViewBuilder private var tables: some View {
        if let d = store.details[backup.id] {
            if !d.tables.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    let (n, rows) = (d.tables.count, d.totalRows.formatted())
                    Text(t("Tables · \(n) tables, \(rows) rows", "Bảng · \(n) bảng, \(rows) dòng")).font(.headline)
                    Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 3) {
                        ForEach(d.tables.prefix(25), id: \.self) { table in
                            GridRow {
                                Text(table.name).font(.callout.monospaced())
                                Text(table.rows.formatted()).font(.callout).monospacedDigit().gridColumnAlignment(.trailing)
                            }
                        }
                    }
                    if n > 25 {
                        Text(t("… and \(n - 25) more tables", "… và \(n - 25) bảng khác")).font(.caption).foregroundStyle(.secondary)
                    }
                }
            } else if backup.primary.kind == .dump {
                Text(t("pg_dump custom format: restored with pg_restore.", "Định dạng custom của pg_dump: khôi phục bằng pg_restore."))
                    .font(.callout).foregroundStyle(.secondary)
            }
        } else {
            ReadStatus(backup: backup)
        }
    }
}

/// Reading progress or error of one backup, or the button that downloads one only in S3.
struct ReadStatus: View {
    @Environment(Store.self) private var store
    let backup: Backup

    var body: some View {
        if let e = store.detailErrors[backup.id] {
            Text(e).foregroundStyle(.red).textSelection(.enabled)
        } else if store.inspecting.contains(backup.id) {
            HStack {
                ProgressView().controlSize(.small)
                Text(backup.hasLocal ? t("Reading database.sql…", "Đang đọc database.sql…")
                                     : t("Downloading from S3 and reading…", "Đang tải từ S3 và đọc…"))
                    .foregroundStyle(.secondary)
            }
        } else if !backup.hasLocal {
            let size = formatBytes(backup.primary.size)
            Button { Task { await store.inspect(backup) } } label: {
                Label(t("Read contents (downloads \(size) from S3)", "Đọc nội dung (tải \(size) từ S3)"),
                      systemImage: "icloud.and.arrow.down")
            }
        }
    }
}

/// Row counts of this backup against another of the same tenant.
struct CompareSection: View {
    @Environment(Store.self) private var store
    let backup: Backup
    @State private var otherID: Backup.ID?

    private var others: [Backup] { store.currentTenant?.backups.filter { $0.id != backup.id } ?? [] }

    /// The backup just before this one, or else the one after.
    private var defaultOther: Backup.ID? {
        let all = store.currentTenant?.backups ?? []
        guard let i = all.firstIndex(where: { $0.id == backup.id }) else { return nil }
        return all.indices.contains(i + 1) ? all[i + 1].id : (i > 0 ? all[i - 1].id : nil)
    }

    var body: some View {
        if !others.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(t("Compare with", "So sánh với")).font(.headline)
                    Picker("", selection: $otherID) {
                        ForEach(others) { Text(dateText($0.date) + ($0.hasLocal ? "" : " · S3")).tag(Optional($0.id)) }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
                content
            }
            .onAppear { otherID = defaultOther }
            .onChange(of: backup.id) { otherID = defaultOther }
            .task(id: otherID) {
                if let o = otherID.flatMap(store.backup), o.hasLocal { await store.inspect(o) }
            }
        }
    }

    @ViewBuilder private var content: some View {
        if let other = otherID.flatMap(store.backup) {
            if let mine = store.details[backup.id], let theirs = store.details[other.id] {
                let older = (other.takenAt ?? 0) < (backup.takenAt ?? 0)
                ComparisonView(
                    comparison: older ? Comparison(old: theirs, new: mine) : Comparison(old: mine, new: theirs),
                    from: older ? other : backup, to: older ? backup : other)
            } else {
                if store.details[backup.id] == nil, !backup.hasLocal {
                    Text(t("Read this backup first.", "Cần đọc bản này trước.")).foregroundStyle(.secondary)
                }
                if store.details[other.id] == nil { ReadStatus(backup: other) }
            }
        }
    }
}

struct ComparisonView: View {
    let comparison: Comparison
    let from: Backup
    let to: Backup

    var body: some View {
        let c = comparison
        let delta = Int64(c.rowsAfter) - Int64(c.rowsBefore)
        VStack(alignment: .leading, spacing: 8) {
            let span = "\(dateText(from.date)) → \(dateText(to.date)): \(c.rowsBefore.formatted()) → \(c.rowsAfter.formatted())"
            Text(t("\(span) rows (\(signed(delta)))", "\(span) dòng (\(signed(delta)))"))
                .foregroundStyle(.secondary)
            let emptied = c.changes.filter(\.emptied)
            if !emptied.isEmpty {
                let names = emptied.map(\.name).joined(separator: ", ")
                Label(t("\(emptied.count) tables lost all their rows: \(names)", "\(emptied.count) bảng mất hết dữ liệu: \(names)"),
                      systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
            }
            if c.changes.isEmpty {
                Label(t("Every table has the same number of rows", "Số dòng mọi bảng giống nhau"), systemImage: "equal.circle")
                    .foregroundStyle(.secondary)
            } else {
                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 3) {
                    GridRow {
                        Text(t("Table", "Bảng"))
                        Text(t("Before", "Trước")).gridColumnAlignment(.trailing)
                        Text(t("After", "Sau")).gridColumnAlignment(.trailing)
                        Text("").gridColumnAlignment(.trailing)
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    ForEach(c.changes.prefix(40)) { change in
                        GridRow {
                            Text(change.name).font(.callout.monospaced())
                            Text(change.before.map { $0.formatted() } ?? "—").monospacedDigit()
                            Text(change.after.map { $0.formatted() } ?? "—").monospacedDigit()
                            Text(change.before == nil ? t("new", "bảng mới") : change.after == nil ? t("dropped", "bị bỏ") : signed(change.delta))
                                .monospacedDigit()
                                .foregroundStyle(change.emptied ? .red : change.delta < 0 ? .orange : .green)
                        }
                        .font(.callout)
                    }
                }
                Text([c.changes.count > 40 ? t("… and \(c.changes.count - 40) more changed", "… và \(c.changes.count - 40) bảng đổi khác") : nil,
                      t("\(c.unchanged) unchanged", "\(c.unchanged) bảng không đổi")].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func signed(_ n: Int64) -> String { n > 0 ? "+\(n.formatted())" : n.formatted() }
}

struct RestoreSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let backup: Backup
    @State private var database = ""
    @State private var replace = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(t("Restore \(backup.tenant)", "Khôi phục \(backup.tenant)")).font(.title3.bold())
            let (date, kind, target) = (dateText(backup.date), backup.primary.kindLabel, store.target.detail)
            Text(t("\(date) · from \(kind) · into \(target)", "\(date) · từ \(kind) · vào \(target)"))
                .foregroundStyle(.secondary)
            if store.target.via == .server {
                Label(t("This is a PostgreSQL server, not a local container.", "Đây là server Postgres, không phải container local."),
                      systemImage: "server.rack")
                    .foregroundStyle(.orange)
            }

            if let job = store.job {
                progress(job)
            } else {
                form
            }
        }
        .padding(20)
        .frame(width: 500)
        .onAppear {
            store.resetJob()
            database = store.defaultDatabaseName(backup)
        }
    }

    private var exists: Bool { store.databaseExists(database) }

    @ViewBuilder private var form: some View {
        TextField(t("Database name", "Tên database"), text: $database)
            .textFieldStyle(.roundedBorder)
            .font(.body.monospaced())
        if exists {
            if store.target.canDrop {
                Toggle(t("Overwrite the existing database \(database)", "Ghi đè database \(database) đang có"), isOn: $replace)
                    .foregroundStyle(.orange)
            } else {
                Text(t("Database \(database) already exists and this server does not allow overwriting it: pick another name.",
                       "Database \(database) đã có. Server này không cho ghi đè: đặt tên khác."))
                    .foregroundStyle(.orange)
            }
        }
        Text(backup.hasLocal
             ? t("Only the database is restored; the files/ folder is not.", "Chỉ nạp database; thư mục files/ không được khôi phục.")
             : t("Read straight from S3, downloading only up to the end of database.sql. The files/ folder is not restored.",
                 "Đọc thẳng từ S3, chỉ tải tới hết database.sql. Thư mục files/ không được khôi phục."))
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        HStack {
            Spacer()
            Button(t("Cancel", "Huỷ")) { dismiss() }.keyboardShortcut(.cancelAction)
            Button(t("Restore", "Khôi phục")) { Task { await store.restore(backup, into: database, replace: replace) } }
                .keyboardShortcut(.defaultAction)
                .disabled(database.isEmpty || (exists && !(replace && store.target.canDrop)))
        }
    }

    @ViewBuilder private func progress(_ job: JobStatus) -> some View {
        if job.finished {
            if let error = job.error {
                Label(errorText(error), systemImage: "xmark.octagon.fill").foregroundStyle(.red).textSelection(.enabled)
            } else {
                Label(t("Restored into \(database)", "Đã khôi phục vào \(database)"), systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
            }
            HStack {
                if job.error == nil {
                    Button(t("Copy database name", "Copy tên database")) {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(database, forType: .string)
                    }
                }
                Spacer()
                Button(t("Done", "Xong")) { dismiss() }.keyboardShortcut(.defaultAction)
            }
        } else {
            ProgressView(value: job.fraction) {
                Text(phaseText(job.phase))
            } currentValueLabel: {
                Text(job.total > 0 ? "\(formatBytes(job.bytes)) / \(formatBytes(job.total))" : " ").monospacedDigit()
            }
            HStack {
                Spacer()
                Button(t("Stop", "Dừng"), role: .destructive) { store.cancelRestore() }
            }
        }
    }
}

struct DatabasesView: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var pendingDrop: Database?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading) {
                    Text(t("Databases in \(store.target.label)", "Database trong \(store.target.label)")).font(.title3.bold())
                    Text(store.target.detail).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button { Task { await store.loadDatabases() } } label: { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.borderless)
                    .help(t("Reload", "Tải lại"))
            }
            if let e = store.databaseError {
                Text(e).foregroundStyle(.red).textSelection(.enabled)
            }
            Table(store.databases) {
                TableColumn(t("Name", "Tên")) { Text($0.name).font(.body.monospaced()) }
                TableColumn(t("Size", "Dung lượng")) { Text(formatBytes($0.size)).monospacedDigit() }.width(100)
                TableColumn("") { db in
                    Button(role: .destructive) { pendingDrop = db } label: { Image(systemName: "trash") }
                        .buttonStyle(.borderless)
                        .disabled(!store.target.canDrop)
                }
                .width(30)
            }
            .frame(height: 360)
            HStack {
                let total = formatBytes(store.databases.reduce(0) { $0 + $1.size })
                Text(t("Total \(total)", "Tổng \(total)")).foregroundStyle(.secondary)
                if !store.target.canDrop {
                    Text(t("· this server does not allow dropping", "· server này không cho xoá")).foregroundStyle(.secondary)
                }
                Spacer()
                Button(t("Close", "Đóng")) { dismiss() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 580)
        .confirmationDialog(t("Drop database \(pendingDrop?.name ?? "")?", "Xoá database \(pendingDrop?.name ?? "")?"),
                            isPresented: .init(get: { pendingDrop != nil }, set: { if !$0 { pendingDrop = nil } })) {
            Button(t("Drop permanently", "Xoá vĩnh viễn"), role: .destructive) {
                if let db = pendingDrop { Task { await store.drop(db) } }
            }
        } message: {
            Text(t("On \(store.target.detail). Open connections to this database are closed.",
                   "Trên \(store.target.detail). Các kết nối đang mở tới database này sẽ bị ngắt."))
        }
    }
}

// MARK: settings

struct SettingsView: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var editingBucket: BucketConfig?
    @State private var editingTarget: TargetConfig?

    var body: some View {
        VStack(spacing: 0) {
            TabView {
                general.tabItem { Text(t("General", "Chung")) }
                folders.tabItem { Text(t("Folders", "Thư mục")) }
                buckets.tabItem { Text("S3") }
                targets.tabItem { Text("Postgres") }
            }
            .padding([.horizontal, .top], 16)
            HStack {
                Spacer()
                Button(t("Done", "Xong")) {
                    dismiss()
                    Task { await store.refresh() }
                }
                .keyboardShortcut(.defaultAction)
            }
            .padding(16)
        }
        .frame(width: 600, height: 480)
        .sheet(item: $editingBucket) { BucketEditor(config: $0) }
        .sheet(item: $editingTarget) { TargetEditor(config: $0) }
    }

    private var general: some View {
        @Bindable var store = store
        @Bindable var lang = Lang.shared
        return Form {
            Picker(t("Language", "Ngôn ngữ"), selection: $lang.choice) {
                Text(t("Same as the system", "Theo hệ thống")).tag(Language.system)
                Text("English").tag(Language.en)
                Text("Tiếng Việt").tag(Language.vi)
            }
            Section(t("Restored databases", "Database khôi phục")) {
                TextField(t("Name", "Tên"), text: $store.dbTemplate, prompt: Text(Store.defaultTemplate))
                    .font(.body.monospaced())
                LabeledContent(t("Example", "Ví dụ")) {
                    Text(Store.databaseName(template: store.dbTemplate, tenant: "shop", date: .now)).font(.body.monospaced())
                }
                Text(t("{tenant} is the tenant, {date} the backup date (yyyymmdd), {time} its time (hhmm). For example myapp_{tenant}_{date}.",
                       "{tenant} là tên tenant, {date} là ngày sao lưu (yyyymmdd), {time} là giờ (hhmm). Ví dụ myapp_{tenant}_{date}."))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section(t("Cleaning up", "Dọn bản cũ")) {
                Stepper(t("Keep the newest \(store.keep) per tenant", "Giữ \(store.keep) bản mới nhất mỗi tenant"),
                        value: $store.keep, in: 1...20)
                Text(t("Older backups on this Mac go to the Trash. Copies in S3 are never touched.",
                       "Bản cũ hơn trên máy được chuyển vào Thùng rác. Bản trên S3 không bị đụng tới."))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }

    private var folders: some View {
        Form {
            Section {
                ForEach(store.folders, id: \.self) { folder in
                    HStack {
                        Image(systemName: "folder")
                        Text(folder).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button { store.folders.removeAll { $0 == folder } } label: { Image(systemName: "minus.circle") }
                            .buttonStyle(.borderless)
                            .help(t("Remove", "Bỏ"))
                    }
                }
            } footer: {
                HStack {
                    Text(t("Backups in these folders are found on every scan.", "Bản sao lưu trong các thư mục này được tìm mỗi lần quét."))
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button(t("Add folder…", "Thêm thư mục…"), action: addFolder)
                }
            }
        }
        .formStyle(.grouped)
    }

    private var buckets: some View {
        Form {
            Section {
                if store.buckets.isEmpty {
                    Text(t("No buckets yet.", "Chưa có bucket nào.")).foregroundStyle(.secondary)
                }
                ForEach(store.buckets) { b in
                    HStack {
                        Image(systemName: "cloud")
                        VStack(alignment: .leading) {
                            Text(b.label)
                            Text(b.endpoint).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button(t("Edit", "Sửa")) { editingBucket = b }
                        Button { store.removeBucket(b) } label: { Image(systemName: "minus.circle") }
                            .buttonStyle(.borderless)
                            .help(t("Remove", "Bỏ"))
                    }
                }
            } footer: {
                HStack(alignment: .top) {
                    Text(t("Backups are read from {prefix}{tenant}/{file}.", "Bản sao lưu được đọc từ {prefix}{tenant}/{tên file}."))
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button(t("Add bucket…", "Thêm bucket…")) { editingBucket = BucketConfig() }
                }
            }
        }
        .formStyle(.grouped)
    }

    private var targets: some View {
        Form {
            Section {
                ForEach(store.targets) { target in
                    HStack {
                        Image(systemName: target.via == .docker ? "shippingbox" : "server.rack")
                        VStack(alignment: .leading) {
                            Text(target.label)
                            Text(target.detail + (target.via == .server && !target.allowDrop ? t(" · create only", " · chỉ tạo mới") : ""))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button(t("Edit", "Sửa")) { editingTarget = target }
                        Button { store.removeTarget(target) } label: { Image(systemName: "minus.circle") }
                            .buttonStyle(.borderless)
                            .help(t("Remove", "Bỏ"))
                            .disabled(store.targets.count < 2)
                    }
                }
            } footer: {
                HStack {
                    Spacer()
                    Button(t("Add container…", "Thêm container…")) { editingTarget = TargetConfig() }
                    Button(t("Add server…", "Thêm server…")) { editingTarget = TargetConfig(via: .server, host: "", port: 5432) }
                }
            }
        }
        .formStyle(.grouped)
    }

    private func addFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        guard panel.runModal() == .OK else { return }
        for url in panel.urls where !store.folders.contains(url.path) {
            store.folders.append(url.path)
        }
    }
}

/// The result of "Check", shared by both editors.
private struct CheckResult: View {
    let result: Result<String, Error>?

    var body: some View {
        switch result {
        case .success(let s): Label(s, systemImage: "checkmark.circle.fill").foregroundStyle(.green)
        case .failure(let e): Label(e.localizedDescription, systemImage: "xmark.octagon.fill").foregroundStyle(.red)
                .textSelection(.enabled)
        case nil: EmptyView()
        }
    }
}

struct BucketEditor: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State var config: BucketConfig
    @State private var secret = ""
    @State private var checking = false
    @State private var result: Result<String, Error>?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Form {
                Section {
                    TextField("Endpoint", text: $config.endpoint, prompt: Text("s3.amazonaws.com"))
                    TextField(t("Port", "Cổng"), value: $config.port, format: .number.grouping(.never))
                    Toggle("TLS (https)", isOn: $config.useSSL)
                    TextField("Region", text: $config.region, prompt: Text("us-east-1"))
                    TextField("Bucket", text: $config.bucket)
                    TextField("Prefix", text: $config.prefix, prompt: Text(t("optional", "tuỳ chọn")))
                    TextField("Access key", text: $config.accessKey)
                    SecureField("Secret key", text: $secret)
                } header: {
                    Text(t("S3 bucket", "Bucket S3")).font(.title3.bold())
                } footer: {
                    // A grouped form's footer is trailing-aligned otherwise.
                    VStack(alignment: .leading, spacing: 8) {
                        Text(t("Backups are read from {prefix}{tenant}/{file}; folders starting with _ are skipped. The secret key is kept in the Keychain. Backstack only reads from the bucket.",
                               "Bản sao lưu được đọc từ {prefix}{tenant}/{tên file}; thư mục bắt đầu bằng _ được bỏ qua. Secret key lưu trong Keychain. App chỉ đọc, không ghi hay xoá gì trên bucket."))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        CheckResult(result: result)
                    }
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .formStyle(.grouped)
            HStack {
                Button(t("Check", "Kiểm tra")) { Task { await check() } }.disabled(checking)
                if checking { ProgressView().controlSize(.small) }
                Spacer()
                Button(t("Cancel", "Huỷ")) { dismiss() }.keyboardShortcut(.cancelAction)
                Button(t("Save", "Lưu")) { save() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(config.endpoint.isEmpty || config.bucket.isEmpty)
            }
            .padding(16)
        }
        .frame(width: 480, height: 520)
        .onAppear { secret = Keychain.get(config.id) }
    }

    private func check() async {
        checking = true
        Keychain.set(config.id, secret)
        result = await store.checkBucket(config).map { t("Connected · \($0) objects", "Kết nối được · \($0) object") }
        checking = false
    }

    private func save() {
        Keychain.set(config.id, secret)
        if let i = store.buckets.firstIndex(where: { $0.id == config.id }) {
            store.buckets[i] = config
        } else {
            store.buckets.append(config)
        }
        dismiss()
    }
}

struct TargetEditor: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State var config: TargetConfig
    @State private var password = ""
    @State private var running: [PGContainer] = []
    @State private var checking = false
    @State private var result: Result<String, Error>?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Form {
                Section {
                    TextField(t("Display name", "Tên hiển thị"), text: $config.name,
                              prompt: Text(config.via == .docker ? config.container : t("e.g. staging", "vd. staging")))
                    if config.via == .docker {
                        HStack {
                            TextField(t("Container", "Container"), text: $config.container)
                            Menu(t("Running", "Đang chạy")) {
                                ForEach(running, id: \.self) { c in
                                    Button("\(c.name) · \(c.image)") { config.container = c.name }
                                }
                                if running.isEmpty { Text(t("No PostgreSQL containers running", "Không thấy container Postgres nào")) }
                            }
                            .fixedSize()
                        }
                        TextField("User", text: $config.user)
                    } else {
                        TextField("Host", text: $config.host)
                        TextField(t("Port", "Cổng"), value: $config.port, format: .number.grouping(.never))
                        TextField("User", text: $config.user)
                        SecureField(t("Password", "Mật khẩu"), text: $password)
                        Picker("SSL", selection: $config.sslmode) {
                            ForEach(["disable", "prefer", "require", "verify-full"], id: \.self) { Text($0) }
                        }
                        Toggle(t("Allow dropping and overwriting databases", "Cho phép xoá và ghi đè database"), isOn: $config.allowDrop)
                    }
                } header: {
                    Text(config.via == .docker ? t("PostgreSQL in Docker", "Postgres trong Docker") : t("PostgreSQL server", "Server Postgres"))
                        .font(.title3.bold())
                } footer: {
                    // A grouped form's footer is trailing-aligned otherwise.
                    VStack(alignment: .leading, spacing: 8) {
                        if config.via == .server {
                            Text(t("Uses the psql and pg_restore installed on this Mac (Homebrew or Postgres.app). The password is kept in the Keychain. Without permission to drop, Backstack only creates new databases and never drops or overwrites one.",
                                   "Dùng psql và pg_restore trên máy này (Homebrew hoặc Postgres.app). Mật khẩu lưu trong Keychain. Tắt quyền xoá thì app chỉ tạo database mới, không bao giờ xoá hay ghi đè database có sẵn."))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        CheckResult(result: result)
                    }
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .formStyle(.grouped)
            HStack {
                Button(t("Check", "Kiểm tra")) { Task { await check() } }.disabled(checking)
                if checking { ProgressView().controlSize(.small) }
                Spacer()
                Button(t("Cancel", "Huỷ")) { dismiss() }.keyboardShortcut(.cancelAction)
                Button(t("Save", "Lưu")) { save() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(config.via == .docker ? config.container.isEmpty : config.host.isEmpty)
            }
            .padding(16)
        }
        .frame(width: 480, height: config.via == .docker ? 340 : 480)
        .onAppear { password = Keychain.get(config.id) }
        .task { running = (try? await Task.detached { try Core.containers() }.value) ?? [] }
    }

    private func check() async {
        checking = true
        Keychain.set(config.id, password)
        result = await store.checkTarget(config).map { t("Connected · \($0) databases", "Kết nối được · \($0) database") }
        checking = false
    }

    private func save() {
        Keychain.set(config.id, password)
        if let i = store.targets.firstIndex(where: { $0.id == config.id }) {
            store.targets[i] = config
        } else {
            store.targets.append(config)
        }
        if config.id == store.targetID { store.selectTarget(config.id) }
        dismiss()
    }
}
