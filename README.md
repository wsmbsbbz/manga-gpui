# manga-gpui

用 [gpui-kit](https://github.com/longbridge/gpui-kit)（GPUI）写的桌面漫画阅读器。第一版支持本地书库。站点源通过 `ComicSource` trait 接入，拷贝漫画的接入说明见 [handoff/copymanga/README.md](handoff/copymanga/README.md)。

## 运行

```bash
cargo run --release
```

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
| `MANGA_DEBUG_ROUTE` | 启动时直接打开页面：`<漫画>`、`<漫画>/<章节>/<页>`，或 `@history`、`@search` |

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
- 来源切换、按 `Capabilities` 隐藏不支持的功能、运行时可配置的 `SourceKey`（接入站点源时一起做）。
- 书库内符号链接的规范化。
- 详情页章节列表虚拟化（章节数上千时再做）。
- 搜索结果分页。
- 阅读 `.cbz` 时每页都会重新打开压缩包；缩放后不保留页内位置。
