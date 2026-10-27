# RSTerm

基于 Rust 和 GPUI Kit 的 SSH / SFTP 桌面工作台，提供保存的主机库、多标签终端、双栏文件管理和当前主机检查面板。

当前以 **macOS** 为主要开发与验收目标，尚未验证 Windows / Linux 原生客户端。监控首版支持本地 macOS 和远程 Linux；这不等同于所有平台已完成支持。

## 构建与启动

工具链由 `rust-toolchain.toml` 固定为 Rust **1.99.1**，依赖由 `Cargo.lock` 固定。准备对应平台的原生构建环境后运行：

```sh
cargo build --locked
cargo run --locked
```

- **macOS**：Xcode Command Line Tools；发布包最低系统版本为 macOS 15.0，Apple Silicon 与 Intel 分别构建。
- **Windows x64**：Visual Studio 的 C++ 桌面开发工具、MSVC 和 Windows SDK；release shader 编译需要 SDK 中的 `fxc.exe`。Actions 会自动定位并检查它。
- **Linux x64**：GNU C/C++ 构建工具、pkg-config，以及 Fontconfig/FreeType、XKB、X11/XCB、Wayland、D-Bus 等开发库；工作流中列出了 Ubuntu 24.04 的完整安装命令。运行需要可用的桌面会话、GPU 驱动及对应系统库，不是无图形环境的服务端程序。

应用默认不会连接主机，也不会在启动时读取 SSH Config 或 shell 历史。连接、导入和历史读取均需主动操作。

## GitHub 自动构建与下载

`.github/workflows/build.yml` 在推送到 `main`、提交 Pull Request、手动运行、推送 `v*` 标签或发布 Release 时构建。普通构建提供 Actions Artifacts；发布构建在所有质量检查和四目标构建成功后，将程序包及 SHA-256 附加到对应 Release，不再因为 Release 已存在而直接失败。首次云端构建仍需推送后在 Actions 中确认，配置存在不代表四个平台已经验证通过。

GitHub 自动显示的 **Source code (zip / tar.gz)** 只是源码快照，不是可运行程序。二进制在工作流成功后才会出现在 Release 的 Assets 中，源码归档仍会保留。

完整测试与严格 Clippy 在 macOS 执行；Windows / Linux 先编译包括测试代码在内的所有 targets，再构建 release，不执行仍含 macOS 按键或 Unix 路径假设的全套测试。四目标编译成功也不等于这些平台的原生界面、输入法和连接功能已经验收。

### 首次推送与手动构建

1. 推送前检查 `git status --short` 和实际差异，把新增的 `src/` 模块、`scripts/`、工作流、工具链文件及 `Cargo.lock` 一起提交，避免漏掉尚未跟踪的源码。
2. 不要使用未经检查的 `git add .`；排除个人 `.claude` 设置、主机配置、密码、私钥和构建缓存。当前已跟踪的个人文件也需要单独检查，`.gitignore` 不会自动取消跟踪。
3. 推送后进入仓库 **Actions → Build binaries → Run workflow**，选择分支，保持 `release_tag` 为空并运行。PR 和 `main` 推送也会自动执行。不填标签时只生成 Artifacts，不修改 Release。
4. 等待质量检查与四个平台的构建 job 完成，在运行页面底部的 **Artifacts** 下载对应目标。Artifacts 保留 30 天；下载后先解开 GitHub 外层归档，里面包含程序包及同名 `.sha256` 校验文件。

| 使用的机器 | 目标 | 程序包 |
| --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` | `rsterm-{版本}-{目标}.app.zip` |
| macOS Intel | `x86_64-apple-darwin` | `rsterm-{版本}-{目标}.app.zip` |
| Windows x64 | `x86_64-pc-windows-msvc` | `rsterm-{版本}-{目标}.zip` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `rsterm-{版本}-{目标}.tar.gz` |

macOS 解压后打开 `RSTerm.app`；Windows 解压后启动 `rsterm.exe`；Linux 解压后在桌面会话运行 `./rsterm`。Linux 包基于 Ubuntu 24.04 / glibc 2.39 构建，仍依赖系统动态库，不是静态通用包或 AppImage。

**macOS 包只有 ad-hoc 签名，没有 Developer ID 签名或 Apple 公证；Windows 包也没有发行者签名。** 系统可能阻止或警告运行未知来源的软件，不应把这些包描述为已受系统信任。自动构建不需要个人访问令牌、签名证书或账户密码。

### Release 自动附加二进制

先把工作流、`scripts/` 等修改提交并推送到 `main`。标签必须严格对应**该标签提交**的 `Cargo.toml` 版本，例如 `0.0.1` 对应 `v0.0.1`。支持以下入口：

- **GitHub 页面手动发布**：创建并发布 Release（正式版或预发布版）后，`release: published` 自动构建该标签，成功后补齐 Assets。仅保存草稿不会触发此事件。新标签应包含本次工作流；给旧标签发布 Release 不会自动采用 main 后来新增的触发器。
- **推送新标签**：标签 push 自动构建；如果没有对应 Release，创建附带二进制的 **Draft Release**，由你检查、下载试运行后发布。已有 Release 保持原来的公开/草稿状态。
- **补齐已经存在的 Release**：进入 **Actions → Build binaries → Run workflow**，选择包含新工作流的 `main`，在 **release_tag** 填写 `v0.0.1`，再运行。无需删除 Release 或标签；成功后刷新 Release 页面查看 Assets。如果对应 Release 不存在，只创建草稿，不自动公开。

例如，准备好发布的新提交可以主动执行：

```sh
git tag v0.0.1
git push origin v0.0.1
```

不要为已经存在的标签重复执行创建命令或强行移动标签。手动补传虽然从 main 启动工作流，**编译的仍是填写标签的原提交**，不是 main 最新代码；各 job 使用同一提交 SHA，上传前再次核对标签。旧标签代码或仓库状态本身无法构建时，需要修复并正常发布新版本，不能把 main 的二进制冒充旧标签版本。

重复运行只补缺失附件，不更改 Release 的标题、说明或公开状态，也不覆盖/删除已有附件。完整的归档与校验文件对会先验证后保留；如果只上传了一半且与本次构建不一致，工作流明确失败，避免错误配对。**如果仓库启用了不可变 Release，公开后缺失的附件不能补传，必须先将二进制附加到草稿，再发布。**

默认工作流只有读取仓库权限，只有明确发布标签的 `Attach release binaries` job 获得 `contents: write`，PR 不进入该 job。如果仓库或组织禁止工作流写入，发布 job 会失败，需要仓库管理员允许这一权限；普通 Artifacts 构建不需要写权限。

### 本地打包

先构建本机 release 二进制，再使用与 CI 相同的打包入口，例如 macOS Apple Silicon：

```sh
MACOSX_DEPLOYMENT_TARGET=15.0 cargo build --release --locked
python3 scripts/package.py --target aarch64-apple-darwin --version 0.0.1 \
  --binary target/release/rsterm --output-dir dist
python3 -m unittest discover -s scripts -p 'test_package.py'
```

打包器仅收集明确指定的二进制和生成的应用元数据，不包含主机库、设置、快捷键配置、`known_hosts`、私钥、`.claude` 或 Cargo 缓存。不同平台的包应在对应原生系统构建，不要仅更换目标名称给另一种架构的二进制打包。

## 连接与主机管理

- 点击「新建连接」填写地址、端口、用户名和认证方式；支持密码、私钥，Unix 平台另支持 SSH Agent。当前锁定的 SSH 库在 Windows 不支持 Agent，请使用密码或私钥。
- 首次连接需要确认服务器指纹。已保存指纹发生变化，或信任库无法安全读取时，不会自动放行。
- 主机可保存、编辑、复制、搜索，并放入嵌套文件夹；支持拖动到文件夹或根目录，空文件夹也会保留。
- 使用主机列表中的 SSH Config 导入入口，主动读取 `~/.ssh/config`，预览、补齐必要信息并选择后导入应用主机库。源文件保持只读，导入后独立保存，不自动同步。
- 导入不是完整的 OpenSSH 配置执行器：不会运行 `ssh -G`、`Match exec` 或代理命令；不支持的 `Include`、`Match`、跳板/代理等配置会提示或阻止导入，不能靠补齐表单绕过。
- 记住的密码和私钥口令使用系统凭据存储，不写入普通 TOML；私钥路径与主机元数据可以保存。

## 终端与 SFTP

- 「本地终端」打开本机 shell；SSH 与本地会话都使用独立标签。
- 活动标签使用独立表面、强调标记和较重文字；同名不同目标补充目标信息，同一目标重复打开使用稳定的 `#1`、`#2` 等会话序号。序号不会因关闭其他标签重新编号。
- 标签位置编号随排列变化，对应数字导航；`1…8` 直达相应位置，`9` 始终到最后一个标签。
- 终端支持 ANSI / 256 色、UTF-8、滚动历史、选区、剪贴板、IME 与窗口尺寸更新。聚焦终端时，`Tab` / `Shift-Tab` 发给 shell，`Ctrl-C` 保持中断语义，不是复制。
- SSH 标签可打开「SFTP 文件」：本地与远程双栏浏览、上传/下载、进度与取消，以及新建目录、重命名、删除文件/空目录和修改权限。
- 传输先写临时文件再发布；失败或取消不应破坏已有目标。远端服务器不支持安全原子覆盖时，会拒绝该覆盖操作，不采用先删除旧文件再改名的方式。
- 设置内置 Tokyo Night、Catppuccin Mocha、Nord、Catppuccin Latte 四套主题，并提供字体、终端及连接选项。

跳板机/代理、任意终端分屏、端口转发、同步输入、命令收藏执行和断点续传不属于当前已实现功能。

## 快捷键

通过右侧「快捷键帮助」入口查看**本次运行实际生效**的按键与作用范围；按钮提示也使用同一份配置。下表为默认值，配置后以应用内列表为准。

| 操作 | macOS | 其他平台默认值 |
| --- | --- | --- |
| 新建 SSH 连接 | `Cmd-N` | `Ctrl-Shift-N` |
| 新建本地终端 | `Cmd-T` | `Ctrl-Shift-T` |
| 关闭当前标签 | `Cmd-W` | `Ctrl-Shift-W` |
| 设置 | `Cmd-,` | `Ctrl-Shift-,` |
| 退出 | `Cmd-Q` | `Ctrl-Shift-Q` |
| 下一个 / 上一个标签 | `Ctrl-Tab` / `Ctrl-Shift-Tab` | 相同 |
| 标签 1…8 / 最后一个标签 | `Cmd-1…8` / `Cmd-9` | `Alt-1…8` / `Alt-9` |
| SFTP 文件 | `Cmd-Shift-F` | `Ctrl-Alt-Shift-F` |
| 系统 / 网络 / 历史 | `Cmd-Shift-I` / `Cmd-Shift-N` / `Cmd-Shift-H` | `Ctrl-Alt-Shift-I` / `N` / `H` |
| 聚焦终端 / 主机搜索 | `Cmd-Shift-T` / `Cmd-Shift-L` | `Ctrl-Alt-Shift-T` / `L` |
| 重连已断开的会话 | `Cmd-Shift-R` | `Ctrl-Alt-Shift-R` |
| 快捷键帮助 | `Cmd-/` | `Ctrl-Alt-Shift-/` |
| 终端复制 / 粘贴 / 全选 | `Cmd-C` / `Cmd-V` / `Cmd-A` | `Ctrl-Shift-C` / `V` / `A` |
| 移动主机或文件夹 | `Cmd-Shift-M` | `Ctrl-Alt-Shift-M` |

终端编辑动作仅在终端内生效，移动动作仅在主机树内生效。SFTP 快捷键不作用于输入框，以保留输入框自身的查找/替换编辑行为。弹窗打开时暂停应用动作；输入法组合输入期间不切换当前终端。其他平台的默认值是配置设计，**不是原生验收结论**。

### 配置文件

macOS 默认路径：

```text
~/Library/Application Support/rsterm/keybindings.toml
```

设置与快捷键帮助会显示实际绝对路径，并提供「复制路径」；点击后显示约 2 秒「已复制」，期间仍可再次复制。复制内容是完整路径，不带额外引号或换行。可在启动前用 `RSTERM_CONFIG_DIR` 指定另一个应用配置目录；该目录同时影响主机库和普通设置，不只是快捷键。

文件缺失时创建默认内容；已有文件及其注释不会因保存普通设置而被改写。**编辑后需要重启应用，不支持热重载。**

- `version = 1` 是必需字段。
- `[bindings]` 中省略动作：继承内置默认值。
- 非空数组：替换该动作的全部默认键，可提供多个候选键。
- 空数组 `[]`：禁用该动作的快捷键，不删除界面入口。
- 仅支持单步按键，不支持多步 chord；每动作最多 8 个绑定，文件最多 64 KiB。
- `secondary` 在 macOS 表示 Cmd，在其他平台表示 Ctrl；别名仍受安全校验。
- 裸字符、普通 shell 控制键、输入编辑冲突、未知动作及重叠范围内的重复绑定会被拒绝。
- 解析或校验失败时，**整份覆盖不生效**，本次使用默认快捷键并提示路径、位置和原因；原文件保留，不输出配置正文。

示例（动作名与当前 catalog 一致；未列出的动作继承默认值）：

```toml
version = 1

[bindings]
next_tab = ["ctrl-tab", "f13"]
previous_tab = ["ctrl-shift-tab", "f14"]
show_system = ["secondary-shift-i"]
show_history = []
```

完整动作名可在首次生成的配置文件中查看，例如 `new_connection`、`new_local_terminal`、`close_tab`、`select_tab_1`、`select_last_tab`、`toggle_files`、`focus_terminal`、`focus_host_search`、`reconnect_session`、`terminal_copy`、`terminal_paste`、`terminal_select_all`、`move_host_node` 和 `show_shortcuts`。

## 当前主机检查面板

右侧「系统」「网络」「历史」跟随**当前标签对应的应用连接账户**：本地标签查看本机，SSH 标签查看那台远程机器。它不会跟踪终端中再次运行的 `ssh`、`sudo` 或 tmux 内部环境。面板始终显示当前目标。

宽窗口使用可调整的侧栏；空间不足时替代工作区主体，保留标签，并提供「返回终端」。切换布局不会新建终端或断开 SSH，原有 SFTP 会话也保留。

### 系统与网络

- 默认关闭；只有打开相应页面才采集，不轮询后台标签。
- 系统页在一次请求完成后约 2 秒再采样；网络连接页约 5 秒刷新。首帧 CPU / 接口速率需要预热，无有效差分时显示未知而不是 0。
- 系统信息包括可用的 OS、架构、运行时间、CPU 型号/逻辑核心、内存及接口收发速率。
- 网络页显示 TCP / UDP 状态、本地/远端数字地址，以及当前权限可见的进程名和 PID，不采集完整进程命令行；最多显示 1000 行，搜索在内存完成。
- 本地 macOS 使用 `sysinfo` 和固定 `/usr/sbin/lsof` 调用；远程 Linux 使用 `/proc`、文本形式的发行版信息及 `ss`。不会做 DNS 查询、提权或自动安装工具。
- 远程检查复用已认证 SSH，使用独立的无 PTY exec 通道，不把采集脚本输入交互终端。服务端仍可能通过账户 shell 处理 exec；强制命令、启动输出、缺少工具或权限不足可能使检查不可用。
- 关闭、切页、切标签、断开或重连会使旧请求失效。失败不会触发 SSH 自动重连；临时错误会标记已有数据过期。若远端迟迟不确认或关闭检查通道，检查功能会等待清理，不反复创建新通道，也不为此关闭用户连接。

### 已保存的 shell 历史

1. 打开「历史」，选择 Bash `~/.bash_history` 或 Zsh `~/.zsh_history`。
2. 主动点击「读取」或「刷新」。仅打开页面、切换来源或切换主机都不会自动读取历史。
3. 在当前内存快照内搜索，选择记录查看预览，再明确选择「复制」或「填入终端」。列表 Enter / 双击不会自动执行。

读取范围与限制：

- 本地历史仅读取 Unix 平台的 Bash/Zsh 默认文件；Windows 本地历史明确不支持。仅读取当前账户的默认已写盘文件，不记录键盘输入，不运行交互 `history`，不探测自定义 `HISTFILE`，不读取其他账户历史，不修改 shell 配置。当前 shell 尚未写盘的命令可能缺失。
- 仅读取普通文件，拒绝符号链接、FIFO、设备等特殊来源，并检查实际打开的文件描述符。远程安全读取需要已有的 `python3`；缺失时显示工具不可用，不自动安装或降级为不安全读取。
- 有界读取最多最近 2 MiB，保留最近 1000 条记录，单条最多 64 KiB；截断或不完整边界会被省略或标记。文件可能同时被 shell 更新，不能将结果视为原子历史快照。
- 保留可解析的时间戳、重复记录和顺序，最新记录优先。普通 Bash 历史缺少可靠多行边界时按保存行展示，**仅允许复制，不允许填入**；不宣称能还原所有 shell 多行语法。
- 控制字符、双向文本控制符等用可见转义预览。无效编码的记录不会被有损解码后当作“原文”复制；多行记录只能明确复制，不能填入。
- 填入仅接受完整、有效 UTF-8、单行且不含控制字符/危险行分隔符的记录，并再次核对目标、会话与快照；shell 未就绪、IME 正在组合输入或终端处于备用屏幕时拒绝。**不会追加回车。**
- 填入前请自行确认已返回 shell 提示符。应用无法可靠判断任意前台程序，不能保证文本不会被该程序解释。
- 历史正文只保存在内存，不建立缓存或备份，不写入应用日志。离开历史页、关闭面板、切换来源/目标或断开后释放。只有主动「复制」才写入系统剪贴板；剪贴板可能由系统或其他应用保留。

## 开发验证与已知限制

```sh
cargo fmt --check
cargo check --all-targets --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked
git diff --check
```

需要外部 SFTP 服务器的集成测试默认忽略，应在隔离服务与测试凭据下另行运行。协议测试和 headless UI 测试不能替代真实 macOS 窗口、键盘、IME 与深浅主题验收；测试不应连接个人主机或读取个人历史文件。

已知 macOS 原生问题：AppKit Touch Bar 的 `_NSTouchBarFinderObservation` 在移除 `nextResponder` 观察者时可能抛出 `NSRangeException` 导致退出。该问题尚未修复，不应以关闭无障碍、吞掉异常或绕过系统权限作为解决方案。

技术栈：GPUI Kit、Tokio、russh / russh-sftp、alacritty_terminal、portable-pty、TOML 与系统凭据存储。
