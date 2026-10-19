# 跃传图标

青绿色圆角底板搭配象牙白 / 浅薄荷绿的两枚折叠箭头，表达电脑之间轻松、双向传输文件。大色块和清晰留白便于在 Dock、任务栏和小尺寸列表里辨认。

唯一源文件为 `ui/brand/icon.png`。应用侧栏和 README 直接引用该源文件；Mac / Windows 图标由 Tauri CLI 派生：

```sh
bash scripts/generate-icons.sh
```

脚本只保留 `src-tauri/icons/` 中需要的桌面图标，临时生成的移动端资源留在 `target/` 并自动清理。更新图标后需重新编译和打包，操作系统的图标缓存可能需要重启应用后更新。

源图使用内置 imagegen 生成，透明背景。提示词如下：

```text
Use case: logo-brand. Create ONE final production app icon for LeapDrop, a simple Mac/Windows LAN file-transfer app. Square 1024x1024 RGBA image. A beautifully proportioned deep pine / petrol teal rounded-square tile centered with modest even transparent margins (tile occupies roughly 84% of canvas), crisp superellipse-like corners. Inside is an original bold minimal symbol made from TWO opposing folded arrow ribbons: one ivory-white ribbon moves northeast, another light mint ribbon moves southwest, with a clean dark negative-space diagonal gap between them. Together they feel like two files effortlessly leaping between computers, a subtle dynamic L idea, balanced and easily readable at 32px. Swiss graphic-design precision, confident broad geometric forms, softly rounded internal corners, clean flat vector-like finish. Restrained subtle teal color variation on tile only, mostly flat. Straight-on, absolutely no perspective, no scene, no mockup, no text, no lettering, no wordmark, no border, no extra circles, no sparkle, no glass, no glossy plastic, no noisy texture, no tiny details, no watermark. Must feel like a refined independent desktop utility, simple and friendly. Original identity, do not imitate any existing app logo. Output ONLY the icon tile with transparent outside, ready to be used directly as application icon.
```
