import Foundation

// ---------------------------------------------------------------------------
// 批次历史（阶段 E 规格 §1.1 / §1.3）
//
// 「记住用过的交付批次」这件事的全部**纯计算**都在这个文件里：增 / 更新 / 去重 /
// 排序 / 淘汰 / 解析 / 序列化。它**不碰文件系统**（那是 `BatchHistoryStore` 的事），
// 也没有时钟（时间由调用方以 `Date` 传进来）—— 于是它整份都是可断言的，
// 也**不可能**碰到人类伙伴真实的 `~/Library/Application Support/`。
//
// 全局约束 E-7：可断言的纯计算落 `Presentation/` 并带单测；
// E-1（坏了也不影响启动）、E-3（上限 50）、E-4（权威只有一个）在本文件里的落点：
//   · E-1 ⇒ [`BatchHistory.parse`] **没有 `throws`**：读不出来 / 解析失败 /
//           `version` 不认识一律当空历史（"失败"这条路径根本不存在）；
//   · E-3 ⇒ [`BatchHistory.maximumEntries`]（50）+ [`BatchHistory.recording`] 的淘汰；
//   · E-4 ⇒ [`BatchHistory.mostRecent`] 是"上次用的码"的**唯一**来源
//           （`AppModel.rememberedCode()` 只读它，内核的 `last_code` 只在它为空时播种）。
// ---------------------------------------------------------------------------

/// 历史里的一条：一个用过的交付码 + 一句备注 + 它是从哪个交付服务器加载的 + 上次用的时间。
///
/// ⚠️ `code` 是**唯一键**（规格 §1.1）：同码只留一条，再次使用是**更新**而不是新增。
/// ⚠️ `lastUsedAt` 保存的是**原文**（ISO 8601 带 `+08:00`，秒级），与交付清单的
///    `created_at` 同形 —— 壳**不把它解析成 `Date` 再格式化回去**（那会在往返里丢掉
///    原文的形状），显示一律复用既有的 [`TimestampPresentation`]。
public struct BatchHistoryEntry: Equatable, Sendable {
    public let code: String
    /// 一句自由文本（人类伙伴选定）。**空串合法** = 没写备注。
    public let note: String
    /// 这一批是从哪个交付服务器加载的。**空串 = 默认服务器**（规格 §1.1）。
    public let baseURL: String
    /// ISO 8601 带偏移的原文，与清单的 `created_at` 同形。
    public let lastUsedAt: String

    public init(code: String, note: String = "", baseURL: String = "", lastUsedAt: String) {
        self.code = code
        self.note = note
        self.baseURL = baseURL
        self.lastUsedAt = lastUsedAt
    }

    /// 要发给内核的那个值：**空串 ⇒ `nil`**（请求里不出现 `base_url` 这个键，
    /// 由内核用自己的默认交付服务器）。与 E-5 是同一条纪律：
    /// 不要显式传一个"和默认一样"的值 —— 那会在默认值变化时静默分叉。
    public var baseURLOrNil: String? {
        baseURL.isEmpty ? nil : baseURL
    }
}

/// 一批历史记录（值类型，不可变）。
///
/// **不变量**（构造函数与每次修改都维持它，所以 `parse(serialize(x)) == x` 恒成立）：
///   ① `entries` 按 `lastUsedAt` **倒序**（最近用的在最前）；
///   ② `code` 唯一；
///   ③ 条数 ≤ [`maximumEntries`]（超出淘汰**最久未用**的）。
///
/// ⚠️ **不变量在这里维持、而不是留给视图**：换码面板只负责把 `entries` 从上到下画出来。
///    一旦"顺序"变成视图的责任，第二个读这个类型的地方（下次自动加载、搜索、导出）
///    就得自己再排一遍 —— 那正是同一条规则写两遍的开始。
public struct BatchHistory: Equatable, Sendable {
    /// 给未来的自己的版本号：**读不出来或不认识 ⇒ 当空历史**（E-1）。
    public static let version = 1

    /// 条数上限（规格 §1.1 明写 50）。无限增长的文件是留给下一个人踩的雷（E-3）。
    public static let maximumEntries = 50

    /// 空历史。**它不是错误态** —— 第一次运行、文件被删、文件坏了都是它（E-1）。
    public static let empty = BatchHistory(entries: [])

    public let entries: [BatchHistoryEntry]

    /// 归一化：排倒序 → 去重（同码留最近那条）→ 截到上限。
    ///
    /// ⚠️ 排序**稳定**（同刻的两条保持传入的相对次序）：拿一个不保证稳定的
    ///    `sorted(by:)` 会让"同一秒里用的两个码"顺序随机，而那是可观察的。
    public init(entries: [BatchHistoryEntry]) {
        let ordered = entries.enumerated()
            .sorted { a, b in
                let (x, y) = (Self.usedAt(a.element), Self.usedAt(b.element))
                return x == y ? a.offset < b.offset : x > y
            }
            .map(\.element)

        var seen = Set<String>()
        var kept: [BatchHistoryEntry] = []
        kept.reserveCapacity(ordered.count)
        for entry in ordered where !entry.code.isEmpty {
            if seen.insert(entry.code).inserted { kept.append(entry) }
        }
        self.entries = Array(kept.prefix(Self.maximumEntries))
    }

    public var isEmpty: Bool { entries.isEmpty }

    /// **"上次用的码"的唯一来源**（E-4）：历史里最近使用的那一条。
    public var mostRecent: BatchHistoryEntry? { entries.first }

    // MARK: - 修改（都返回新值）

    /// 记下一次**成功**的使用（规格 §1.1「同码只留一条，再次使用 ⇒ 更新」）。
    ///
    /// - Parameters:
    ///   - note: `nil`（默认）= **保持这一条原有的备注**。这一条是承重的：
    ///     用户写下的"客户张三"不该因为又用了一次这个码而消失。
    ///     传空串则是显式把它清空。
    ///   - date: 由调用方给（纯函数不读时钟）。
    public func recording(code: String,
                          baseURL: String = "",
                          note: String? = nil,
                          at date: Date) -> BatchHistory {
        // 空码不是一条交付批次：放进去只会变成历史列表里一个点了没反应的假条目。
        guard !code.isEmpty else { return self }
        let fresh = BatchHistoryEntry(code: code,
                                      note: note.map(Self.normalizedNote)
                                          ?? entries.first { $0.code == code }?.note ?? "",
                                      baseURL: baseURL,
                                      lastUsedAt: Self.timestamp(date))
        // 其余条目原样留下（它们的相对次序由 `init` 的排序保证）。
        return BatchHistory(entries: [fresh] + entries.filter { $0.code != code })
    }

    /// 改一条的备注（换码面板里就地编辑那一行）。
    ///
    /// ⚠️ 对一个**不在历史里**的码设备注是**空操作**，不会凭空造出一条没有时间戳的记录
    ///    （历史列表只对屏上那些行提供编辑）。清空备注是传空串，**不是**删掉这一条。
    public func settingNote(_ note: String, forCode code: String) -> BatchHistory {
        guard entries.contains(where: { $0.code == code }) else { return self }
        let normalized = Self.normalizedNote(note)
        return BatchHistory(entries: entries.map {
            guard $0.code == code else { return $0 }
            return BatchHistoryEntry(code: $0.code, note: normalized,
                                     baseURL: $0.baseURL, lastUsedAt: $0.lastUsedAt)
        })
    }

    // MARK: - 读写（磁盘形状）

    /// 从文件内容解析（E-1：**任何**问题都当空历史，绝不抛）。
    ///
    /// 逐层容错：
    ///   · 不是 JSON / 顶层不是对象 / 没有 `version` / `version` 不等于 [`version`]
    ///     / `entries` 不是数组 ⇒ **整份当空**（这些都是"这份文件不是我们写的"）；
    ///   · 某一行不是对象、或没有非空的 `code`（它是主键）⇒ **只丢那一行**，
    ///     其余照留（一行坏了就让用户"重启后什么都没了"是过度反应）；
    ///   · 某一行的 `last_used_at` 解析不出来 ⇒ 该行**留着**，只是排到最末
    ///     （它是排序键，不是主键）。
    ///
    /// ⚠️ 用 `JSONSerialization` 而不是 `Codable`：`Codable` 的合成解码器遇到一个
    ///    类型不对的字段就**整份**失败，而这里要的恰恰是"逐行容错"（见上）。
    public static func parse(_ data: Data) -> BatchHistory {
        guard !data.isEmpty,
              let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let version = root["version"] as? Int, version == Self.version,
              let rows = root["entries"] as? [Any]
        else { return .empty }

        return BatchHistory(entries: rows.compactMap(entry(from:)))
    }

    /// 同上（给测试与调用方一个不用自己转 `Data` 的入口）。
    public static func parse(_ text: String) -> BatchHistory {
        parse(Data(text.utf8))
    }

    /// 落盘的形状（规格 §1.1，字段名逐字）。
    ///
    /// ⚠️ 空串字段（`note` / `base_url`）**照样写出去**：形状是固定的，
    ///    "键有时在有时不在"会让下一个读这个文件的人（包括未来版本的壳）多写一个分支。
    public func serialized() throws -> Data {
        let rows: [[String: Any]] = entries.map {
            ["code": $0.code, "note": $0.note,
             "base_url": $0.baseURL, "last_used_at": $0.lastUsedAt]
        }
        return try JSONSerialization.data(withJSONObject: ["version": Self.version, "entries": rows],
                                          options: [.prettyPrinted, .sortedKeys,
                                                    .withoutEscapingSlashes])
    }

    // MARK: - 行 → 条目

    private static func entry(from any: Any) -> BatchHistoryEntry? {
        guard let row = any as? [String: Any],
              let code = row["code"] as? String, !code.isEmpty
        else { return nil }
        return BatchHistoryEntry(code: code,
                                 note: row["note"] as? String ?? "",
                                 baseURL: row["base_url"] as? String ?? "",
                                 lastUsedAt: row["last_used_at"] as? String ?? "")
    }

    // MARK: - 时间

    /// 交付服务器的固定偏移（`+08:00`）。
    ///
    /// ⚠️ **有意不用本机时区**（E-8，理由写在这里）：交付清单的 `created_at` 是
    ///    `+08:00` 形状的，而 [`TimestampPresentation.text`] 显示时**丢掉偏移、读的是墙上时间**
    ///    —— 若按本机时区写，同一个文件在两个时区的机器上会显示成两个时间
    ///    （"有效期/上次使用"是客户与业务方对账的依据，不能随机器的设置变）。
    ///    固定偏移也让测试与机器时区无关。
    private static let deliveryServerOffsetSeconds = 8 * 3600

    /// `Date` → ISO 8601 带 `+08:00`、**秒级**（与清单的 `created_at` 同形）。
    static func timestamp(_ date: Date) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]      // 秒级、带偏移，不要小数秒
        formatter.timeZone = TimeZone(secondsFromGMT: deliveryServerOffsetSeconds)
            ?? TimeZone(identifier: "UTC") ?? .current
        return formatter.string(from: date)
    }

    /// 排序键。**解析不出来 ⇒ `.distantPast`**（排到最末，最先被淘汰的位置）。
    ///
    /// ⚠️ 每次调用现造 formatter、**不共享一个 `static let`**：`ISO8601DateFormatter`
    ///    不是 `Sendable` 的，一个共享实例在并发调用下就是数据竞争
    ///    （`CoreJSON.decoder` 出于同一条理由也是"每次都造一个新的"）。
    private static func usedAt(_ entry: BatchHistoryEntry) -> Date {
        let raw = entry.lastUsedAt
        guard !raw.isEmpty else { return .distantPast }

        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        if let date = plain.date(from: raw) { return date }

        // 清单里的 `created_at` 实测带小数秒（`…T16:13:34.805751+08:00`），
        // 而我们自己写出去的是秒级 —— 两种都要认。
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: raw) ?? .distantPast
    }

    /// 备注是"**一句**自由文本"（人类伙伴选定）：换行/回车/制表折成空格、首尾空白去掉。
    /// 内部原有的空格**一个都不动** —— 那是他自己写的排版，壳不替他重排。
    ///
    /// ⚠️ `\r\n`（Windows 换行）**折成一个空格**、不是两个：它是**一个**换行。
    ///    把粘贴进来的文本变成"每个换行多一个空格"是那种一眼看不出、却到处都是的脏数据。
    private static func normalizedNote(_ raw: String) -> String {
        let unfolded = raw
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
        var oneLine = ""
        oneLine.reserveCapacity(unfolded.count)
        for character in unfolded {
            switch character {
            case "\n", "\t": oneLine.append(" ")
            default: oneLine.append(character)
            }
        }
        return oneLine.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
