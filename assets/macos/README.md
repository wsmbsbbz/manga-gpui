# Manga GPUI 图标

`AppIcon.icon` 是正式源文件，可用 Icon Composer 打开。Default 使用暖杏背景，Dark 使用暖炭背景；两种外观共用 `Assets/ghost-reading.png`，保留 D 方案的纸感幽灵和橙色漫画书。前景关闭 Liquid Glass，背景遮罩和最终投影由 Apple 工具生成。

请通过仓库根目录的 `scripts/package-macos.sh` 编译，不要把 PNG 或原始 `.icon` 单独复制进应用包来替代 `Assets.car`。脚本保留 actool 输出的静态兼容资源，并合并其图标元数据。

验收：16px 保留幽灵和书的轮廓，32px 能认出幽灵在读打开的书，64px 可辨认眼睛和漫画分格；检查 Finder、Dock、⌘Tab 和应用退出后的显示。macOS 26+ 的图标样式由系统独立控制，应用内部主题不覆盖它。兼容旧系统时需额外实测，不能仅以 `.icns` 存在为依据。

## 素材来源

使用内置 imagegen 工具，从用户选定的 D 方案提取完整透明前景，没有使用 CLI/API 回退。最终编辑提示词如下：

> Use case: background-extraction. Edit target: the provided D macOS icon image. Produce ONLY the complete ivory ghost reading the open orange manga book, including eyes, both hands, cream pages and comic panel design, as a high quality transparent PNG asset. Preserve the exact selected character identity, down-looking eyes, silhouette, pose, proportions, orange/cream palette and restrained paper-cut tactile style. Remove the entire peach rounded-square tile, white outer presentation background, tile shadow, all surrounding backdrop. All pixels outside the ghost and book must be genuinely transparent. Keep delicate internal paper layering/contact shadows, but remove large external cast shadows and heavy texture. Center the complete character without clipping any hands or hem, use a square canvas, with the character occupying roughly 76% of canvas width and 80% of canvas height. This is one foreground layer for Icon Composer: no icon tile, no rounded-square mask, no text, no surrounding mockup. Do not redesign or add objects.
