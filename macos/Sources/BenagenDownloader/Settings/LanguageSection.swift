import SwiftUI
import BenagenCoreKit

// ---------------------------------------------------------------------------
// 设置页的「界面语言」段 —— Windows 壳那边对应 `windows/web/index.html` 的
// `#set-language-section`（同一份三档：跟随系统 / 中文 / English）。
//
// 三条纪律（与设置页其余段落一致）：
//   1. **值来自内核**：`model.settings.language` 是内核手里那一份（`"system"/"zh"/"en"`），
//      本视图不自己存一份"当前语言"——否则保存别的参数时会把语言覆盖掉。
//   2. **整份回传**：改语言走的就是 `applySettings`，它把七项参数 + 语言一起发出去；
//      漏一格就等于重置（`SettingsForm.settings` 是唯一组装点）。
//   3. **立刻生效**：`applySettings` 之后 `AppModel` 会再问一次 `hello`，拿内核解析出的
//      `zh`/`en` 落到 `language`，所以整壳文案（含本段自己的标签）当场换语言。
// ---------------------------------------------------------------------------

/// 语言三档在界面上的显示名。
///
/// ⚠️ **选项名不翻译**：「中文」「English」各自永远以自己的语言显示（客户在英文界面里
///    也要能一眼认出中文那一项）；只有"跟随系统"这一项跟着界面语言走。
private func label(for value: String, lang: Lang) -> String {
    switch value {
    case "zh": return lang.t(.languageChinese)
    case "en": return lang.t(.languageEnglish)
    default: return lang.t(.languageFollowSystem)
    }
}

struct LanguageSection: View {
    @ObservedObject var model: AppModel

    /// 一次改动在飞（设置文件写盘 + 内核整份替换，通常几十毫秒）。
    @State private var applying = false

    private var current: String { model.settings?.language ?? "system" }

    var body: some View {
        Section {
            Picker(model.language.t(.languageTitle), selection: Binding(
                get: { current },
                set: { apply($0) }
            )) {
                ForEach(SettingsForm.languageChoices, id: \.self) { value in
                    Text(label(for: value, lang: model.language)).tag(value)
                }
            }
            .pickerStyle(.menu)
            .disabled(applying || model.settings == nil)
            if applying {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text(model.language.t(.languageNote))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        } footer: {
            Text(model.language.t(.languageNote))
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    /// 把新语言写进设置（**整份**：七项参数原样带上）。
    private func apply(_ value: String) {
        guard var next = model.settings, next.language != value else { return }
        next.language = value
        applying = true
        Task {
            defer { Task { @MainActor in applying = false } }
            do {
                try await model.applySettings(next)
            } catch {
                // 失败**不静默**：`AppModel` 已把原文写进 `lastError`（主区横幅会显示），
                // 这里只把"在飞"收掉、让 Picker 回到内核手里那一份。
                await MainActor.run { applying = false }
            }
        }
    }
}
