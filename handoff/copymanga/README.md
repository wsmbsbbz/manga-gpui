# Handoff：拷贝漫画源（manga-source-copy）

> 状态：已实现。见 [`crates/manga-source-copy`](../../crates/manga-source-copy)。
> 浏览 / 搜索 / 详情 / 阅读已联网验证通过（签名被接受、列表与详情解析正确、封面加载正常）。
> 登录和收藏未实现（无需在应用内处理密码）。下面保留原始设计说明作为记录。

给接手这项工作的 agent。目标是在不改动阅读器、缓存和 UI 主体的前提下，新增一个实现 `ComicSource` 的站点源。

## 0. 先处理阻塞项（需要用户决定，不要自行绕过）

`api.mangacopy.com` 的 App 接口不接受普通请求，旧的公开请求头会返回 `error`。社区脚本的做法是：
- 用从官方 App 里提取出的密钥做请求签名；
- 伪造设备标识。

这等于冒充官方客户端。**本项目不采用这种方式，也不要把这类密钥或签名算法写进仓库。**

开工前请先向用户确认合法的访问方式，例如：
- 站方提供的正式 API 或授权凭据；
- 用户本人账号下、站方允许的访问方式。

在拿到确认之前，只做下面不依赖鉴权的部分：解析、映射、测试夹具、UI 接入。

## 1. 接入点

| 位置 | 要做的事 |
|---|---|
| `crates/manga-source-copy/` | 新 crate，`impl ComicSource for CopySource`，依赖 `manga-core`、`manga-net`（用 `ImageFetcher::http()` 的 reqwest client，在 `Runtime::spawn` 上执行） |
| `crates/manga-app/src/state.rs` | `registry.register(Arc::new(CopySource::new(...)))` |
| `meta().capabilities` | `search: true`、`cache_images: true`，其余如实声明 |
| 图片 | `pages()` 返回 `ImageRequest::Http { url, headers }`。图片需要的请求头放在这里，不要在 UI 层特殊处理；`cache_key()` 不含请求头 |
| 历史记录里的封面 | `headers` 字段标了 `#[serde(skip_serializing)]`，**不会写入 sqlite**，以免泄露 token。如果站点的封面必须带 Referer 或鉴权头，从历史记录取出的封面会缺少这些头。解决方式：给 `ComicSource` 加一个按需补齐请求头的钩子，或在历史页通过 `detail()` 重新取封面。不要把请求头持久化 |

**UI 现状**：`LibraryView`、`SearchView` 直接使用 `AppState.local` 和 `manga_source_local::KEY`。接入第二个源需要：
- 侧边栏加"来源"切换；
- 书架和搜索改为按当前选中源调用；
- 详情页和阅读器已经按 `Nav` 里的 `source` 字段取源，不用改。

## 2. 数据映射（端点路径和返回结构，来自公开的社区脚本）

返回体都是 JSON，数据在 `results` 字段下。

| trait 方法 | 端点 | 映射 |
|---|---|---|
| `explore(page)` | `GET /api/v3/h5/homeIndex`（只有一页）或 `GET /api/v3/comics?limit=30&offset=…&ordering=-datetime_updated` | 漫画对象 `{path_word, name, cover, author[].name, theme[].name}` → `ComicSummary { id: path_word, title: name, subtitle: 第一作者, cover: Http(cover), tags }`；列表里的项可能包了一层 `{comic: {...}}` |
| `search(q, page)` | `GET /api/v3/search/comic?limit=30&offset=…&q=…&q_type=` | `results.list`、`results.total`；`has_more = offset + len < total` |
| `detail(id)` | `GET /api/v3/comic2/{path_word}` | `results.comic` 包含 `name / cover / brief / author / theme / status.display / datetime_updated`；`results.groups` 是分组映射 `{key: {name, path_word}}` |
| 章节 | `GET /api/v3/comic/{path_word}/group/{group_path_word}/chapters?limit=100&offset=…` | 每个分组一个 `ChapterGroup`，分页拉全，`list[] {uuid, name}` → `Chapter { id: uuid, title: name }` |
| `pages(comic, chapter)` | `GET /api/v3/comic/{path_word}/chapter2/{uuid}` | `results.chapter.contents[].url` 要按 `results.chapter.words[i]` 重新排序：`images[words[i]] = contents[i].url` |

注意事项：
- **限流**：HTTP 状态码 210，message 里含 "Expected available in N seconds"，映射成 `SourceError::RateLimited(N)`。重试由调用方决定，不要在源里阻塞等待几十秒。
- **API 域名**：会变，做成配置项，不要写死。
- **章节数超过 100 时要分页**：社区脚本的旧版本把分页路径写死成了某一部漫画，不要照抄。

## 3. 验收标准

- 解析逻辑做成纯函数，输入 JSON、输出领域模型，用录制的样本 JSON 写单测：
  - 排序重排（`words`）
  - 分组
  - 分页
  - 被包了一层的漫画对象
  - 210 限流
- CI 里不要有访问真实网络的测试。
- 打开详情和阅读走现有流程：图片走 `ImageFetcher`，进入磁盘缓存，第二次打开命中缓存。
- 错误在 UI 上有可读的提示，不能 panic。
- 不在日志或 `Debug` 输出里打印 token 或请求头（`ImageRequest` 的 `Debug` 已经做了脱敏）。
