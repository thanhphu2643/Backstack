import Foundation
import Observation

enum Language: String, CaseIterable, Identifiable {
    case system, en, vi
    var id: String { rawValue }
}

/// The interface language, switched live from the settings.
@Observable
final class Lang: @unchecked Sendable {
    static let shared = Lang()

    var choice: Language {
        didSet { UserDefaults.standard.set(choice.rawValue, forKey: "language") }
    }

    private init() {
        choice = Language(rawValue: UserDefaults.standard.string(forKey: "language") ?? "") ?? .system
    }

    var isVietnamese: Bool {
        switch choice {
        case .vi: true
        case .en: false
        case .system: Locale.preferredLanguages.first?.hasPrefix("vi") ?? false
        }
    }

    var locale: Locale { Locale(identifier: isVietnamese ? "vi_VN" : "en_US") }
}

/// An interface string in the chosen language: English first, then Vietnamese.
/// Views re-render on a switch because reading `Lang.shared` is observed.
func t(_ en: String, _ vi: String) -> String { Lang.shared.isVietnamese ? vi : en }

func dateText(_ date: Date?) -> String {
    guard let date else { return t("unknown date", "không rõ ngày") }
    return date.formatted(Date.FormatStyle(date: .abbreviated, time: .shortened).locale(Lang.shared.locale))
}

/// A restore step reported by the core.
func phaseText(_ phase: String) -> String {
    switch phase {
    case "start": t("Starting", "Bắt đầu")
    case "create": t("Creating the database", "Tạo database")
    case "connect": t("Connecting to S3", "Kết nối S3")
    case "open": t("Opening the backup", "Mở bản sao lưu")
    case "load": t("Loading data", "Nạp dữ liệu")
    case "done": t("Done", "Xong")
    case "failed": t("Failed", "Thất bại")
    default: phase
    }
}

/// Errors from the core are in English; the one a user causes on purpose reads better translated.
func errorText(_ error: String) -> String {
    error == "cancelled" ? t("Stopped", "Đã dừng") : error
}
