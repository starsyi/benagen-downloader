import SwiftUI
import BenagenCoreKit

// ---------------------------------------------------------------------------
// 首启语言窗（规格 W1）—— Windows 壳那边是 `app.js` 的 `maybeAskLanguage`（同一个判据）。
//
// **判据只有一个**：`hello` 回执里的 `language_chosen == false`，也就是
// 「设置文件里还没有 `language` 这一格」（`core/src/settings.rs::load_from_with_flag`）。
// ⚠️ 不是"值等于 system"——客户主动选了「跟随系统」也算选过，重启不许再弹。
//
// 三个选项各自写回设置（整份替换），选完内核重新解析、`language_chosen` 变 true，
// 本窗随绑定自动收起。默认高亮的那一项是内核已经解析出的语言（默认跟随了系统）。
// ---------------------------------------------------------------------------

struct LanguageSheet: View {
    @ObservedObject var model: AppModel

    @State private var applying = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(model.language.t(.languageSheetTitle))
                .font(.title3.weight(.semibold))
            Text(model.language.t(.languageSheetNote))
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 10) {
                ForEach(SettingsForm.languageChoices, id: \.self) { value in
                    Button(label(for: value, lang: model.language)) { apply(value) }
                        .buttonStyle(.bordered)
                        .disabled(applying)
                }
            }
            if applying { ProgressView().controlSize(.small) }
        }
        .padding(24)
        .frame(minWidth: 420)
    }

    private func label(for value: String, lang: Lang) -> String {
        switch value {
        case "zh": return lang.t(.languageChinese)
        case "en": return lang.t(.languageEnglish)
        default: return lang.t(.languageFollowSystem)
        }
    }

    private func apply(_ value: String) {
        guard var next = model.settings else { return }
        next.language = value
        applying = true
        Task {
            defer { Task { @MainActor in applying = false } }
            do { try await model.applySettings(next) } catch {
                await MainActor.run { applying = false }
            }
        }
    }
}
