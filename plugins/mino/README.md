# Mino Codex 插件源码

此目录是 Mino Codex 插件的 canonical、可验证源码，不是可以直接安装的原生产物。目标平台打包会在保持全部 source bytes 与目录结构的同时，加入且只加入一个 `bin/mino` 或 `bin/mino.exe`。

<!-- plugin-contract: one-native-binary -->

## 目录职责

- `.codex-plugin/plugin.json` 描述插件身份和面向 Codex 的能力。
- `launcher.json` 固定 CLI、protocol、Agent schemas/capabilities、内嵌 standards、支持 target、binary 相对路径和 non-interactive probe argv。
- `skills/mino/` 必须与 `assets/skill/mino` 逐字节一致，并由 Rust plugin contract 校验。
- 此 source tree 不包含 `bin/`；binary 只在 target-native packaging 阶段加入。

## 运行边界

Skill 只从自己的 plugin root 解析 launcher 声明的相对 binary。安装和运行不能修改 `PATH`，不能下载替代 binary，不能搜索其他 Mino，也不能访问网络。

<!-- plugin-contract: path-unchanged -->

如果 binary 缺失、平台不匹配、不是普通文件或 capability identity 不兼容，应以 `environment_unavailable` / exit 7 停止，并要求匹配当前平台的完整 artifact。

## 验证与分发

打包前同时运行当前 plugin source validator 与 Rust contract。打包器会把 source、binary、protocol、standards、capabilities、target 和 archive inventory 绑定到 manifest，并从解压后的临时目录执行兼容性 smoke。

此目录本身不会发布、安装或更新插件，也不会创建 marketplace entry。上述动作均属于独立的用户授权流程。

<!-- plugin-contract: no-publish-install-update -->
