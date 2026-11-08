# Manga GPUI

用 [gpui-kit](https://github.com/longbridge/gpui-kit)（GPUI）写的桌面漫画阅读器。支持本地书库和拷贝漫画站点源，左下角「来源」切换。站点源通过 `ComicSource` trait 接入。

> 本项目用于研究 GPUI，仅本地个人使用、不做分发。拷贝漫画源是非官方客户端，多半不符合站点服务条款，请自行斟酌。它按公开的社区配置构造请求头，不涉及账号密钥；客户端保持克制（每进程固定设备标识、不做抓取循环、尊重服务端限流）。

## 运行

```bash
cargo run --release
```

### macOS 应用包

需要完整 Xcode 26+（`xcode-select -p` 应指向该 Xcode 的 Developer 目录）和 Rust 工具链：

```bash
./scripts/package-macos.sh
open "target/release/bundle/Manga GPUI.app"
```

脚本为当前 Mac 架构构建 release，将程序和图标装配为 **Manga GPUI.app**，完成本地 ad-hoc 签名后才替换上一次产物。应用版本来自 Cargo；最低系统版本取自编译后二进制，不代表已经验证所有旧系统。产物可复制到「应用程序」目录使用；本地签名不等于 Developer ID 签名或公证。

图标源位于 [`assets/macos/AppIcon.icon`](assets/macos/AppIcon.icon)，使用同一个幽灵读漫画前景，提供暖杏浅色和暖炭深色背景。macOS 26+ 根据系统的图标样式设置选择外观；该设置与窗口深浅色外观独立。旧系统使用编译器生成的兼容静态图标，需在对应系统单独验证。普通 `cargo run` 不加载应用包图标，查看正式图标请启动 `.app`。应用界面目前仍是深色主题。

首次启动点「选择文件夹」指定书库。目录约定：

```text
书库/
  某部漫画/
    cover.jpg           可选封面
    第1话/001.jpg …     章节文件夹
    第2话.cbz           章节压缩包（.cbz / .zip）
  短篇/001.jpg …        图片直接放在漫画目录 → 单章
  单本.cbz              根目录压缩包 → 单章漫画
```

所有名称按自然序排序（`第2话` < `第10话`）。

### 演示书库与调试开关

```bash
cargo run -p manga-app --example gen_demo -- /tmp/demo-library
MANGA_LIBRARY=/tmp/demo-library cargo run
```

| 环境变量 | 作用 |
|---|---|
| `MANGA_LIBRARY` | 覆盖已保存的书库路径 |
| `MANGA_READER_MODE` | `scroll` / `ltr` / `rtl` |
| `MANGA_DEBUG_ROUTE` | 启动时直接打开页面：`<漫画>`、`<漫画>/<章节>/<页>`，或 `@history`、`@search`（`<漫画>` 取当前来源） |
| `MANGA_SOURCE` | 启动时选中的来源：`local` / `copy_manga` / `manhuagui` |
| `MANGA_COPY_API` | 覆盖拷贝漫画 API 域名（镜像会变） |
| `MANGA_COPY_TOKEN` | 可选，已有的登录 token（本应用不实现登录流程） |

### 拷贝漫画源

左下角「来源」选「拷贝漫画」。书架显示最近更新（最多 5 页），用搜索查找具体作品。实现范围：浏览 / 搜索 / 详情 / 阅读；登录和收藏未实现。API 域名默认 `api.copy2000.online`，失效时用 `MANGA_COPY_API` 或配置文件 `copy_api_host` 指定新镜像。请求签名方案见 [`crates/manga-source-copy/src/sign.rs`](crates/manga-source-copy/src/sign.rs) 顶部说明。

首次访问前，会用同一设备标识向 `api.copy-manga.com` 的 `/api/v3/system/network2?platform=3` 初始化一次，并发请求等待初始化完成；初始化失败可重试。覆盖 API 域名不会改变此初始化地址。HTTP 210 会保留服务端拒绝说明，仅在明确给出限流等待时间时显示秒数。

### 漫画柜源

左下角「来源」选「漫画柜」。书架显示最近更新（`/list/update_pN.html`，最多 5 页），搜索走 `/s/<关键词>_pN.html`。实现范围：浏览 / 搜索 / 详情（多分组：单话、单行本、番外等）/ 阅读；登录、收藏、评论未实现。解析公开网页 HTML，章节页的图片列表藏在打包脚本里，由 [`unpack.rs`](crates/manga-source-manhuagui/src/unpack.rs) 以纯数据方式解码（LZString 关键字表 + 标识符替换），不执行任何 JavaScript。

漫画柜会封禁访问过快的 IP，因此所有网页请求串行、间隔至少 0.8 秒；HTTP 403/429 会提示“可能访问过快”。图片 CDN（`i.hamreus.com`）要求 `Referer: https://www.manhuagui.com/`，请求时自动附带。手动联网检查：`cargo run -p manga-source-manhuagui --example live`。

## 阅读器

模式：条漫连续滚动、单页左→右、单页右→左。阅读模式会记住。

| 按键 | 作用 |
|---|---|
| `←` / `→` | 翻页（右→左模式下方向反转） |
| `Space` / `Shift-Space`、`PgDn` / `PgUp` | 下一屏 / 上一屏 |
| `↑` / `↓` | 小步滚动；放大时平移 |
| `]` / `[` | 下一章 / 上一章 |
| `=` `-` `0`（可加 `⌘`） | 放大 / 缩小 / 重置 |
| `M` | 切换模式 |
| `Enter` | 显示 / 隐藏工具栏 |
| `Esc` | 返回 |

翻页模式下：
- 点击左右 30% 区域翻页，点击中间切换工具栏。
- 放大后可以拖拽平移。
- 滚轮翻页（有防连翻）。
- 底部分段进度条可以点击跳页。

## 架构

```text
crates/
  manga-core          领域模型、ComicSource trait、ImageRequest、自然排序（不依赖 UI）
  manga-net           独立 tokio runtime 与 GPUI 之间的桥接（Pending：drop 即取消）、
                      图片取字节（同 URL 请求合并、远程并发上限、重试退避、磁盘缓存）
  manga-store         磁盘缓存（sha256 键、原子写、按 mtime LRU 淘汰）、
                      阅读历史（rusqlite WAL）、配置
  manga-source-local  本地书库源（文件夹 / cbz、防路径穿越）
  manga-app           gpui-kit UI：书架、搜索、详情、历史、阅读器
```

图片流水线（`manga-app/src/images.rs`）：
1. 取字节。
2. 在后台线程解码，并缩放到屏幕需要的宽度（页面最宽 1800px，封面 420px）。
3. 超过 4096px 的长图切成多块纹理。
4. 解码结果放进按字节计算的 LRU（页面默认 384MB，封面 96MB）。淘汰时调用 `window.drop_image` 释放 GPU 纹理。

加载按优先级调度，同时最多 4 个。阅读器按"离当前页的距离"排优先级：
- 预取后 N 页（默认 5）和前 1 页。
- 读到章末前 3 页时，预取下一章的页面列表和前 2 张图。
- 离开窗口的任务直接取消。

设计 token 在 `manga-app/src/theme.rs`，视图里不直接写颜色和尺寸值。

## 测试

```bash
cargo test --workspace
```

## 已知限制（第一版）

- 只验证了 macOS。
- 双页模式还没做。
- 条漫模式的缩放只在窗口宽度以内调整，不做横向平移。
- 点击和键盘交互没有自动化 UI 测试：gpui-kit 的 `test-support` 特性和 `gpui-fast` 的 `taffy` 版本冲突，依赖解析失败。
- 书库扫描结果有缓存。根目录增删漫画会自动重新扫描；改动漫画文件夹内部（例如新增章节）需要点「刷新」。

代码审查中暂缓到后续版本的项：
- 拷贝漫画的登录与收藏（现仅浏览/搜索/详情/阅读）；运行时可配置的 `SourceKey`（现为内置两源的静态 key）。
- 书库内符号链接的规范化。
- 详情页章节列表虚拟化（章节数上千时再做）。
- 搜索结果分页。
- 阅读 `.cbz` 时每页都会重新打开压缩包；缩放后不保留页内位置。
