# RSTerm 功能设计文档

> 创建日期: 2026-03-21
> 目标定位: 免费、开源、跨平台的现代 SSH 终端，面向全栈用户（开发+运维）

---

## 一、市场现有工具分析

### 1.1 主流 SSH 终端对比

| 功能特性 | Xshell | WindTerm | Termius | Royal TSX | Tabby | RSTerm 目标 |
|---------|--------|----------|---------|-----------|-------|-------------|
| **许可** | 商业/免费版 | 免费/开源 | 商业/免费 | 商业 | 开源 | **开源 MIT** |
| **跨平台** | Windows | 全平台 | 全平台 | macOS | 全平台 | **全平台** |
| **内置 SFTP** | ✅ | ✅ | ✅ | ✅ | ❌ | **✅** |
| **多标签** | ✅ | ✅ | ✅ | ✅ | ✅ | **✅** |
| **分屏** | ✅ | ✅ | ✅ | ✅ | ✅ | **✅** |
| **跳板机** | ✅ | ✅ | ✅ | ✅ | ❌ | **✅** |
| **端口转发** | ✅ | ✅ | ✅ | ✅ | ✅ | **✅** |
| **脚本/自动化** | ✅ | ✅ | ❌ | ✅ | ✅ | **✅** |
| **同步输入** | ✅ | ✅ | ❌ | ✅ | ❌ | **✅** |
| **主题定制** | 一般 | 丰富 | 丰富 | 一般 | 丰富 | **丰富** |
| **启动速度** | 中等 | 快 | 中等 | 慢 | 中等 | **极快** |
| **内存占用** | 中等 | 低 | 中等 | 高 | 中等 | **极低** |
| **密码管理** | 本地加密 | 本地 | 云同步 | 本地 | 本地 | **本地加密** |

### 1.2 差异化定位

**RSTerm 的核心竞争力：**

1. **完全免费开源** - MIT 协议，无任何限制，可商用可二次开发
2. **Rust 原生性能** - 启动 < 1s，内存 < 100MB，长时间运行稳定
3. **全栈场景优化** - 同时满足开发调试和运维管理需求
4. **现代化 UI** - 基于 egui 的高性能原生界面，媲美商业软件质感
5. **隐私优先** - 所有数据本地存储，无云依赖，无遥测

---

## 二、功能模块详单

### 2.1 核心功能（P0 - MVP 必需）

#### 2.1.1 SSH 连接模块

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| SSH2 协议支持 | 基于 russh 库实现完整 SSH2 协议 | P0 |
| 密码认证 | 用户名+密码登录 | P0 |
| 公钥认证 | OpenSSH 私钥文件登录（RSA/ED25519/ECDSA） | P0 |
| 会话保存 | 保存连接配置到本地，支持分组管理 | P0 |
| 快速连接 | 一键连接已保存的会话 | P0 |
| 连接状态 | 实时显示连接状态、延迟、服务器信息 | P0 |
| 断线重连 | 网络中断自动重连，保留会话历史 | P0 |
| 心跳保活 | 定时发送 keep-alive 防止超时断开 | P0 |

#### 2.1.2 终端模块

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| PTY 分配 | 支持标准 pseudo-terminal 分配 | P0 |
| xterm-256color | 支持 256 色终端渲染 | P0 |
| UTF-8 编码 | 完整 UTF-8 字符支持，包括中文、Emoji | P0 |
| 字体配置 | 自定义字体、字号、行高 | P0 |
| 基础快捷键 | 复制/粘贴/清屏/重置 | P0 |
| 滚动缓冲 | 支持回滚查看历史输出（可配置缓冲行数） | P0 |
| 光标样式 | 支持块状/下划线/竖线光标，可配置闪烁 | P0 |

#### 2.1.3 界面框架

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 单窗口多标签 | 顶部标签栏，支持新建/关闭/切换 | P0 |
| 标签右键菜单 | 关闭/关闭其他/复制会话 | P0 |
| 状态栏 | 显示当前连接信息、时间、性能指标 | P0 |
| 主题切换 | 深色/浅色主题 | P0 |

### 2.2 重要功能（P1 - 正式版）

#### 2.2.1 会话管理增强

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 会话分组 | 树形结构管理会话，支持文件夹嵌套 | P1 |
| 会话搜索 | 按名称/主机/标签快速搜索 | P1 |
| 会话标签 | 自定义标签分类（生产/测试/开发等） | P1 |
| 会话导入导出 | 导出为 JSON/TOML，支持跨设备迁移 | P1 |
| 会话克隆 | 快速复制现有会话配置 | P1 |
| 最近连接 | 记录最近连接的会话，快速访问 | P1 |

#### 2.2.2 高级连接功能

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 跳板机 (Jump Host) | 多跳连接，支持多级跳板 | P1 |
| SOCKS5 代理 | 支持 SOCKS5 代理连接 | P1 |
| HTTP 代理 | 支持 HTTP/HTTPS 代理 | P1 |
| 连接超时配置 | 可配置连接/读取/写入超时 | P1 |
| 主机密钥管理 | 首次连接提示，支持已知主机列表 | P1 |
| 证书认证 | OpenSSH 证书登录 | P1 |

#### 2.2.3 终端增强

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 水平/垂直分屏 | 支持多个终端窗格，可拖拽调整大小 | P1 |
| 分屏同步 | 分屏间可同步输入 | P1 |
| 终端搜索 | 在当前终端内搜索文本 | P1 |
| 正则搜索 | 支持正则表达式搜索 | P1 |
| URL 点击识别 | 自动识别 URL，点击打开浏览器 | P1 |
| 文件路径识别 | 识别文件路径，右键复制/打开 | P1 |
| 右键菜单 | 复制/粘贴/搜索/发送命令 | P1 |
| 终端铃声 | 可视/声音提示 | P1 |

#### 2.2.4 SFTP 文件管理

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 双面板界面 | 左侧本地文件，右侧远程文件 | P1 |
| 文件浏览 | 目录导航、文件列表、图标显示 | P1 |
| 拖拽传输 | 拖拽文件实现上传/下载 | P1 |
| 传输队列 | 多文件排队传输，显示进度 | P1 |
| 文件操作 | 新建/删除/重命名/复制/移动 | P1 |
| 权限修改 | 修改文件权限 (chmod) | P1 |
| 文件搜索 | 在当前目录搜索文件 | P1 |
| 收藏夹 | 收藏常用本地/远程目录 | P1 |

#### 2.2.5 效率工具

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 命令历史 | 记录执行过的命令，支持搜索 | P1 |
| 命令收藏 | 收藏常用命令，快速发送 | P1 |
| 代码片段 | 保存代码片段，支持变量替换 | P1 |
| 自动补全 | 基于历史的命令自动补全建议 | P1 |

### 2.3 增强功能（P2 - 专业化）

#### 2.3.1 高级终端功能

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 终端录制 | 录制终端会话为视频/文本 | P2 |
| 终端回放 | 回放录制的会话 | P2 |
| 六边形查看 | 以十六进制查看二进制输出 | P2 |
| 终端截图 | 截取当前终端内容为图片 | P2 |
| Markdown 渲染 | 渲染终端中的 Markdown 内容 | P2 |

#### 2.3.2 端口转发

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 本地转发 (-L) | 将远程端口映射到本地 | P2 |
| 远程转发 (-R) | 将本地端口映射到远程 | P2 |
| 动态转发 (-D) | SOCKS 代理 | P2 |
| 转发管理 | 图形化管理所有转发规则 | P2 |

#### 2.3.3 批量操作

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 同步输入 | 在多个终端同时输入相同命令 | P2 |
| 会话组 | 将多个会话编组，批量操作 | P2 |
| 批量执行 | 预设命令列表，批量执行 | P2 |
| 执行结果汇总 | 收集各会话执行结果 | P2 |

#### 2.3.4 安全增强

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 密码加密存储 | 使用系统密钥环或主密码加密 | P2 |
| SSH Agent 集成 | 支持 SSH Agent 转发 | P2 |
| YubiKey 支持 | 硬件密钥认证 | P2 |
| 审计日志 | 记录所有连接和操作 | P2 |

### 2.4 扩展功能（P3 - 未来规划）

| 功能 | 描述 | 优先级 |
|-----|------|-------|
| 插件系统 | 支持第三方插件扩展 | P3 |
| 脚本引擎 | 内置脚本引擎，支持自动化 | P3 |
| SSH 隧道可视化 | 图形化配置隧道 | P3 |
| 会话同步 | 可选的云端会话同步 | P3 |
| 团队协作 | 共享会话配置 | P3 |
| AI 助手 | 集成 AI 辅助命令生成 | P3 |
| Docker 集成 | 管理 Docker 容器 | P3 |
| K8s 集成 | 管理 Kubernetes 集群 | P3 |

---

## 三、前端设计方案

### 3.1 GUI 框架选型

#### 选项对比

| 框架 | 优点 | 缺点 | 适用场景 |
|-----|------|------|---------|
| **egui** | 轻量、即时模式、跨平台、纯 Rust | 非原生外观、高级组件少 | ✅ 推荐 RSTerm |
| Tauri | 现代 Web 技术、体积小 | 需要 Web 开发经验 | 团队熟悉前端 |
| Iced | Elm 架构、原生风格 | 生态较小 | 追求原生感 |
| Slint | 声明式 UI、设计工具 | 商业许可限制 | 商业项目 |
| Gtk-rs | 原生 Linux 集成 | Windows/macOS 体验差 | Linux 专用 |

**推荐选择：egui**

理由：
- 纯 Rust 实现，与项目技术栈一致
- 即时模式 GUI，适合终端这种频繁刷新的场景
- 跨平台支持良好（eframe 集成）
- 性能优秀，内存占用低
- 社区活跃，文档完善

### 3.2 界面布局设计

```
┌─────────────────────────────────────────────────────────────────┐
│  标题栏（窗口控制 + 应用图标 + 当前会话名）                        │
├─────────────────────────────────────────────────────────────────┤
│  工具栏（新建连接 | 快速连接 | SFTP | 端口转发 | 设置）           │
├─────────────────────────────────────────────────────────────────┤
│  会话面板     │                    主终端区域                      │
│  ┌──────────┐ │  ┌─────────────────────────────────────────────┐ │
│  │ 🔍 搜索   │ │  │                                             │ │
│  │ 📁 生产   │ │  │                                             │ │
│  │   🖥️ web-01│ │  │           终端输出内容                       │ │
│  │   🖥️ db-01 │ │  │                                             │ │
│  │ 📁 测试   │ │  │                                             │ │
│  │   🖥️ test-01│ │  │                                             │ │
│  │ 📁 开发   │ │  └─────────────────────────────────────────────┘ │
│  │   ...    │ │  ┌──────────────────────┬────────────────────────┐│
│  └──────────┘ │  │ 分屏终端 1            │ 分屏终端 2             ││
│               │  │                      │                        ││
│               │  └──────────────────────┴────────────────────────┘│
├───────────────┴─────────────────────────────────────────────────┤
│  状态栏：连接状态 | 服务器信息 | 延迟: 12ms | CPU: 2% | 内存: 45MB │
└─────────────────────────────────────────────────────────────────┘
```

### 3.3 核心界面组件

#### 3.3.1 会话管理面板（左侧）

```rust
struct SessionPanel {
    sessions: Vec<SessionGroup>,     // 会话分组
    search_query: String,             // 搜索关键词
    selected_session: Option<SessionId>,
    expanded_groups: HashSet<GroupId>,
}

struct SessionGroup {
    id: GroupId,
    name: String,
    icon: Icon,                       // 文件夹/云/服务器等图标
    sessions: Vec<Session>,
    sub_groups: Vec<SessionGroup>,
}

struct Session {
    id: SessionId,
    name: String,
    host: String,
    port: u16,
    status: ConnectionStatus,         // Disconnected/Connecting/Connected
    tags: Vec<Tag>,
    last_connected: Option<DateTime>,
}
```

#### 3.3.2 终端渲染区（中央）

```rust
struct TerminalPane {
    session_id: SessionId,
    pty: PtySession,                  // PTY 会话
    buffer: TerminalBuffer,           // 滚动缓冲
    scroll_offset: usize,
    selection: Option<TextSelection>,
    search: Option<SearchState>,
    cursor_style: CursorStyle,
}

struct TerminalBuffer {
    lines: Vec<Line>,
    max_lines: usize,                 // 可配置缓冲大小
    current_line: Line,
}

struct Line {
    cells: Vec<Cell>,
    is_wrapped: bool,
}

struct Cell {
    char: char,
    style: CellStyle,
}

struct CellStyle {
    fg_color: Color,
    bg_color: Color,
    bold: bool,
    italic: bool,
    underline: bool,
    blink: bool,
}
```

#### 3.3.3 SFTP 面板

```rust
struct SftpPanel {
    visible: bool,
    split_ratio: f32,                 // 左右面板比例
    local_panel: FilePanel,
    remote_panel: FilePanel,
    transfer_queue: Vec<TransferTask>,
}

struct FilePanel {
    path: PathBuf,
    entries: Vec<FileEntry>,
    selected: HashSet<PathBuf>,
    sort_by: SortOption,
    view_mode: ViewMode,              // List/Grid
}

struct FileEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: DateTime,
    permissions: Permissions,
}

struct TransferTask {
    id: TransferId,
    source: PathBuf,
    destination: PathBuf,
    direction: TransferDirection,     // Upload/Download
    progress: f32,
    state: TransferState,
}
```

### 3.4 主题系统

```rust
struct Theme {
    name: String,
    colors: ColorScheme,
    fonts: FontConfig,
    spacing: SpacingConfig,
}

struct ColorScheme {
    // 界面颜色
    background: Color,
    foreground: Color,
    primary: Color,
    secondary: Color,
    accent: Color,
    error: Color,
    warning: Color,
    success: Color,

    // 终端颜色（16 色调色板 + 256 色）
    terminal: TerminalColors,
}

struct TerminalColors {
    black: Color,
    red: Color,
    green: Color,
    yellow: Color,
    blue: Color,
    magenta: Color,
    cyan: Color,
    white: Color,
    bright_black: Color,
    bright_red: Color,
    // ... 其他亮色
    // 256 色调色板
    palette_256: [Color; 256],
}
```

### 3.5 交互设计要点

#### 键盘快捷键

| 快捷键 | 功能 |
|-------|------|
| `Ctrl+T` | 新建标签页 |
| `Ctrl+W` | 关闭当前标签页 |
| `Ctrl+Tab` | 切换到下一个标签 |
| `Ctrl+Shift+Tab` | 切换到上一个标签 |
| `Ctrl+Shift+T` | 重新打开关闭的标签 |
| `Ctrl+L` | 切换 SFTP 面板 |
| `Ctrl+Shift+\` | 垂直分屏 |
| `Ctrl+Shift+-` | 水平分屏 |
| `Ctrl+Shift+Arrow` | 切换分屏焦点 |
| `Ctrl+Shift+D` | 同步输入开关 |
| `Ctrl+Shift+S` | 保存当前会话配置 |
| `Ctrl+,` | 打开设置 |
| `F11` | 全屏切换 |
| `Ctrl+Shift+C` | 复制 |
| `Ctrl+Shift+V` | 粘贴 |

#### 鼠标交互

| 操作 | 功能 |
|-----|------|
| 单击 | 聚焦终端 / 选择文件 |
| 双击 | 选中单词 / 打开文件 |
| 三击 | 选中整行 |
| 拖拽 | 选择文本 / 拖拽文件传输 |
| 右键 | 上下文菜单 |
| 中键点击 | 粘贴选中文本 |
| 滚轮 | 滚动终端内容 |
| Ctrl+滚轮 | 缩放字体 |

---

## 四、技术栈确认

### 4.1 核心依赖

```toml
[dependencies]
# 异步运行时
tokio = { version = "1", features = ["full"] }

# SSH 协议
russh = "0.45"
russh-keys = "0.45"
russh-sftp = "2"

# GUI 框架
eframe = "0.29"  # egui + 后端集成
egui = "0.29"
egui_extras = { version = "0.29", features = ["image", "file"] }

# 终端处理
vte = "0.13"           # VT100 解析
termini = "1"          # terminfo 数据库
portable-pty = "0.8"   # 本地 PTY（如需本地终端）

# 序列化
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# 加密存储
keyring = "3"          # 系统密钥环
aes-gcm = "0.10"       # 本地加密

# 日志
tracing = "0.1"
tracing-subscriber = "0.3"

# 其他
anyhow = "1"
thiserror = "1"
dirs = "6"             # 系统目录
chrono = { version = "0.4", features = ["serde"] }
```

### 4.2 项目结构建议

```
rsterm/
├── Cargo.toml
├── src/
│   ├── main.rs                    # 应用入口
│   ├── app.rs                     # 应用状态管理
│   │
│   ├── ssh/                       # SSH 模块
│   │   ├── mod.rs
│   │   ├── client.rs              # SSH 客户端封装
│   │   ├── auth.rs                # 认证处理
│   │   ├── session.rs             # 会话管理
│   │   └── tunnel.rs              # 端口转发
│   │
│   ├── terminal/                  # 终端模块
│   │   ├── mod.rs
│   │   ├── pty.rs                 # PTY 处理
│   │   ├── parser.rs              # VT100 解析
│   │   ├── buffer.rs              # 滚动缓冲
│   │   └── render.rs              # 渲染逻辑
│   │
│   ├── sftp/                      # SFTP 模块
│   │   ├── mod.rs
│   │   ├── client.rs              # SFTP 客户端
│   │   ├── transfer.rs            # 文件传输
│   │   └── browser.rs             # 文件浏览
│   │
│   ├── ui/                        # UI 模块
│   │   ├── mod.rs
│   │   ├── app_ui.rs              # 主界面
│   │   ├── session_panel.rs       # 会话面板
│   │   ├── terminal_view.rs       # 终端视图
│   │   ├── sftp_panel.rs          # SFTP 面板
│   │   ├── settings_dialog.rs     # 设置对话框
│   │   └── themes/                # 主题目录
│   │       ├── mod.rs
│   │       ├── dark.rs
│   │       └── light.rs
│   │
│   ├── config/                    # 配置模块
│   │   ├── mod.rs
│   │   ├── settings.rs            # 应用设置
│   │   ├── sessions.rs            # 会话配置
│   │   └── storage.rs             # 本地存储
│   │
│   ├── crypto/                    # 加密模块
│   │   ├── mod.rs
│   │   └── keyring.rs             # 密钥存储
│   │
│   └── utils/                     # 工具模块
│       ├── mod.rs
│       ├── clipboard.rs
│       └── logging.rs
│
├── assets/                        # 静态资源
│   ├── fonts/
│   ├── icons/
│   └── themes/
│
└── tests/                         # 测试
    ├── ssh_tests.rs
    └── terminal_tests.rs
```

---

## 五、版本规划更新

### v0.1.0 - MVP（最小可用）

**目标：可用的 SSH 终端**

- [ ] SSH2 基础连接
- [ ] 密码认证
- [ ] 公钥认证（RSA/ED25519）
- [ ] 单窗口多标签
- [ ] 基础终端渲染（xterm-256color）
- [ ] 会话保存/加载
- [ ] 字体/主题配置
- [ ] 断线重连

**预计工作量：核心功能**

### v0.2.0 - 功能增强

**目标：日常可用**

- [ ] 会话分组管理
- [ ] 高级认证（跳板机、代理）
- [ ] 水平/垂直分屏
- [ ] 终端搜索
- [ ] 命令历史
- [ ] URL 识别

### v0.3.0 - SFTP 集成

**目标：文件管理**

- [ ] SFTP 双面板界面
- [ ] 文件浏览/操作
- [ ] 拖拽传输
- [ ] 传输进度显示

### v0.4.0 - 效率工具

**目标：提升效率**

- [ ] 代码片段
- [ ] 命令收藏
- [ ] 同步输入
- [ ] 端口转发

### v0.5.0 - 安全增强

**目标：企业可用**

- [ ] 密码加密存储
- [ ] SSH Agent 支持
- [ ] 审计日志
- [ ] 证书认证

### v1.0.0 - 正式版

**目标：生产就绪**

- [ ] 全平台测试
- [ ] 性能优化
- [ ] 文档完善
- [ ] 安装包发布

---

## 六、设计决策

| 决策项 | 选择 | 说明 |
|-------|------|------|
| 本地终端支持 | **远程 SSH + 本地 shell** | 既可连接远程，也可打开本地终端 |
| 多窗口模式 | **仅多标签页，不支持独立窗口** | 所有会话在同一窗口的标签页中，简洁统一 |
| 配置文件格式 | **TOML** | 人类可读，Rust 生态友好 |
| 更新机制 | **手动检查更新** | 菜单提供检查更新选项，提示下载 |
| 密钥存储 | **系统密钥环 + 加密文件回退** | 优先使用系统密钥环，Windows 等平台不支持时回退到加密文件存储 |

---

## 七、系统架构设计

### 7.1 整体架构

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              RSTerm Application                         │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                      Presentation Layer (UI)                     │   │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐           │   │
│  │  │SessionPanel│ │TerminalView│ │ SftpPanel │ │Dialogs   │           │   │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘           │   │
│  │                          │ egui/eframe │                         │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                                    │                                    │
│                                    ▼                                    │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                      Application Layer                           │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │   │
│  │  │SessionManager│  │ TabManager   │  │ CommandQueue │          │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘          │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │   │
│  │  │ThemeManager  │  │ KeyBinding   │  │ UpdateChecker│          │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘          │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                                    │                                    │
│                                    ▼                                    │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                        Service Layer                             │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │   │
│  │  │ SSH Service  │  │SFTP Service  │  │ Local Pty    │          │   │
│  │  │  (russh)     │  │(russh-sftp)  │  │(portable-pty)│          │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘          │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │   │
│  │  │ Terminal Emu │  │ Crypto Store │  │ Config Store │          │   │
│  │  │   (vte)      │  │  (keyring)   │  │   (toml)     │          │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘          │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                                    │                                    │
│                                    ▼                                    │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                       Infrastructure Layer                       │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │   │
│  │  │    Tokio     │  │   Logging    │  │   Platform   │          │   │
│  │  │   Runtime    │  │  (tracing)   │  │   (dirs)     │          │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘          │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### 7.2 核心模块划分

#### 7.2.1 SSH Service（SSH 服务模块）

**职责**：管理所有 SSH 连接的生命周期

```
┌─────────────────────────────────────────┐
│              SSH Service                 │
├─────────────────────────────────────────┤
│  - connect(config) -> Session           │
│  - disconnect(session_id)               │
│  - reconnect(session_id)                │
│  - get_session(session_id) -> Session   │
│  - list_sessions() -> Vec<SessionInfo>  │
├─────────────────────────────────────────┤
│  Internal:                               │
│  - connection_pool: HashMap<Id, Session>│
│  - auth_handler: AuthHandler            │
│  - tunnel_manager: TunnelManager        │
└─────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────┐
│              Session                     │
├─────────────────────────────────────────┤
│  - id: SessionId                        │
│  - config: SessionConfig                │
│  - handle: SSHHandle                    │
│  - status: ConnectionStatus             │
│  - channels: Vec<Channel>               │
│  - created_at: DateTime                 │
│  - last_activity: DateTime              │
└─────────────────────────────────────────┘
```

#### 7.2.2 Terminal Emulator（终端模拟模块）

**职责**：处理终端输入输出、VT100 解析、滚动缓冲

```
┌─────────────────────────────────────────┐
│           Terminal Emulator              │
├─────────────────────────────────────────┤
│  Components:                             │
│  ┌─────────────┐  ┌─────────────┐       │
│  │   Parser    │  │   Buffer    │       │
│  │   (vte)     │  │  (scroll)   │       │
│  └─────────────┘  └─────────────┘       │
│  ┌─────────────┐  ┌─────────────┐       │
│  │   Screen    │  │   Cursor    │       │
│  │  (grid)     │  │  (state)    │       │
│  └─────────────┘  └─────────────┘       │
├─────────────────────────────────────────┤
│  API:                                    │
│  - process_input(data: &[u8])           │
│  - process_output(data: &[u8])          │
│  - get_screen() -> &ScreenGrid          │
│  - scroll(lines: i32)                   │
│  - resize(cols: u16, rows: u16)         │
│  - set_selection(range: Range)          │
└─────────────────────────────────────────┘
```

**数据流**：

```
用户输入 ──▶ KeyHandler ──▶ Terminal.write(input)
                                      │
                                      ▼
远程/本地 ◀── Channel.write() ◀── PTY/SSH Session
                                      │
                                      ▼
远程/本地 ──▶ Channel.read() ──▶ Parser(vte) ──▶ Buffer.update()
                                                    │
                                                    ▼
                                              Screen Grid
                                                    │
                                                    ▼
                                              UI Render
```

#### 7.2.3 SFTP Service（文件传输模块）

**职责**：远程文件浏览和传输

```
┌─────────────────────────────────────────┐
│              SFTP Service                │
├─────────────────────────────────────────┤
│  - open(session_id) -> SftpClient       │
│  - close(sftp_id)                       │
│  - list_dir(path) -> Vec<FileEntry>     │
│  - download(src, dst) -> Transfer       │
│  - upload(src, dst) -> Transfer         │
│  - mkdir/remove/rename/chmod            │
├─────────────────────────────────────────┤
│  Transfer Queue (独立 Tokio 任务):       │
│  ┌─────────────────────────────────┐    │
│  │ Task 1: ████████░░ 80%  file.txt│    │
│  │ Task 2: ░░░░░░░░░░  0%  data.db │    │
│  └─────────────────────────────────┘    │
└─────────────────────────────────────────┘
```

#### 7.2.4 Session Manager（会话管理模块）

**职责**：会话配置的持久化和运行时管理

```
┌─────────────────────────────────────────┐
│            Session Manager               │
├─────────────────────────────────────────┤
│  Config Operations:                      │
│  - save_session(session: SessionConfig) │
│  - load_session(id: SessionId)          │
│  - delete_session(id: SessionId)        │
│  - list_sessions() -> Vec<SessionInfo>  │
│  - import_sessions(path)                │
│  - export_sessions(path)                │
│  - create_group(name)                   │
│  - move_to_group(session_id, group_id)  │
├─────────────────────────────────────────┤
│  Runtime State:                          │
│  - active_sessions: HashMap<Id, Active> │
│  - connection_status: HashMap<Id, Status│
└─────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────┐
│         SessionConfig (TOML)             │
├─────────────────────────────────────────┤
│  [[sessions]]                            │
│  id = "uuid-xxx"                        │
│  name = "web-server-01"                 │
│  host = "192.168.1.100"                 │
│  port = 22                              │
│  username = "admin"                     │
│  auth_type = "key"                      │
│  key_path = "~/.ssh/id_ed25519"         │
│  group = "production"                   │
│  tags = ["web", "nginx"]                │
├─────────────────────────────────────────┤
│  [[groups]]                              │
│  id = "prod-group"                      │
│  name = "生产环境"                       │
│  parent = null                          │
└─────────────────────────────────────────┘
```

#### 7.2.5 Tab Manager（标签管理模块）

**职责**：多标签页和分屏的布局管理

```
┌─────────────────────────────────────────┐
│              Tab Manager                 │
├─────────────────────────────────────────┤
│  - new_tab(session_id) -> TabId         │
│  - close_tab(tab_id)                    │
│  - switch_tab(tab_id)                   │
│  - reorder_tab(from, to)                │
│  - split_tab(tab_id, direction)         │
│  - get_layout() -> TabLayout            │
├─────────────────────────────────────────┤
│  Layout Structure:                       │
│                                          │
│  Tab {                                   │
│    id: TabId,                           │
│    name: String,                        │
│    session_id: SessionId,               │
│    layout: SplitLayout,                 │
│  }                                       │
│                                          │
│  SplitLayout {                          │
│    direction: Horizontal | Vertical,    │
│    ratio: f32,                          │
│    panes: [Pane | SplitLayout],         │
│  }                                       │
│                                          │
│  ┌──────────┬──────────┐               │
│  │  Pane 1  │  Pane 2  │               │
│  │          ├──────────┤               │
│  │          │  Pane 3  │               │
│  └──────────┴──────────┘               │
└─────────────────────────────────────────┘
```

### 7.3 数据模型

#### 7.3.1 核心数据结构

```rust
/// 会话配置（持久化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub id: SessionId,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthConfig,
    pub group: Option<GroupId>,
    pub tags: Vec<String>,
    pub environment: HashMap<String, String>,
    pub startup_command: Option<String>,
    pub keep_alive: KeepAliveConfig,
    pub proxy: Option<ProxyConfig>,
    pub jump_hosts: Vec<JumpHostConfig>,
}

/// 认证配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthConfig {
    Password {
        // 密码不直接存储，使用 keyring
        keyring_id: String,
    },
    Key {
        path: PathBuf,
        passphrase_keyring_id: Option<String>,
    },
    Certificate {
        key_path: PathBuf,
        cert_path: PathBuf,
    },
    Agent,
}

/// 会话分组
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGroup {
    pub id: GroupId,
    pub name: String,
    pub icon: Option<String>,
    pub parent: Option<GroupId>,
    pub collapsed: bool,
}

/// 活动会话（运行时）
#[derive(Debug)]
pub struct ActiveSession {
    pub config: SessionConfig,
    pub status: ConnectionStatus,
    pub connected_at: Option<DateTime<Utc>>,
    pub terminal: TerminalInstance,
    pub sftp: Option<SftpInstance>,
    pub channels: Vec<ChannelInfo>,
}

/// 连接状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}

/// 终端实例
pub struct TerminalInstance {
    pub size: (u16, u16),           // (cols, rows)
    pub buffer: TerminalBuffer,
    pub pty: Box<dyn PtyBackend>,   // SSH 或 Local
    pub selection: Option<Selection>,
    pub search: Option<SearchState>,
    pub scroll_offset: usize,
}

/// PTY 后端 trait
pub trait PtyBackend: Send {
    fn write(&mut self, data: &[u8]) -> Result<()>;
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    fn resize(&mut self, cols: u16, rows: u16) -> Result<()>;
    fn is_alive(&self) -> bool;
}
```

### 7.4 异步架构

```
┌─────────────────────────────────────────────────────────────────┐
│                     Main Thread (UI)                             │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  eframe::run_native()                                    │   │
│  │  - egui 渲染循环                                         │   │
│  │  - 用户输入处理                                          │   │
│  │  - 状态更新                                              │   │
│  └─────────────────────────────────────────────────────────┘   │
│                              │                                   │
│                              │ mpsc channel / async channel     │
│                              ▼                                   │
├─────────────────────────────────────────────────────────────────┤
│                   Tokio Runtime (异步)                           │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐   │
│  │SSH Task    │ │SFTP Task   │ │Local PTY   │ │Update Check│   │
│  │            │ │            │ │Task        │ │Task        │   │
│  │- Connect   │ │- Transfer  │ │- Shell     │ │- HTTP      │   │
│  │- Read/Write│ │- Browse    │ │- Read/Write│ │- Version   │   │
│  │- Reconnect │ │- Queue     │ │            │ │            │   │
│  └────────────┘ └────────────┘ └────────────┘ └────────────┘   │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                  Shared State (Arc<Mutex>)                │   │
│  │  - sessions: HashMap<SessionId, ActiveSession>           │   │
│  │  - transfers: Vec<TransferTask>                          │   │
│  │  - notifications: Vec<Notification>                      │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

**UI 与异步任务的通信模式**：

```rust
// 命令（UI -> 后端）
pub enum Command {
    Connect(SessionId, SessionConfig),
    Disconnect(SessionId),
    Reconnect(SessionId),
    WriteInput(SessionId, Vec<u8>),
    ResizeTerminal(SessionId, u16, u16),
    StartSftp(SessionId),
    TransferFile(SessionId, TransferRequest),
    CheckUpdate,
}

// 事件（后端 -> UI）
pub enum Event {
    Connected(SessionId),
    Disconnected(SessionId, Option<String>),
    ConnectionError(SessionId, String),
    TerminalOutput(SessionId, Vec<u8>),
    TransferProgress(TransferId, f32),
    TransferComplete(TransferId),
    UpdateAvailable(String, String),  // version, download_url
}

// 通道
pub struct AppChannels {
    pub command_tx: mpsc::Sender<Command>,
    pub command_rx: mpsc::Receiver<Command>,
    pub event_tx: mpsc::Sender<Event>,
    pub event_rx: mpsc::Receiver<Event>,
}
```

**UI-异步状态同步详细模式**：

```rust
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use crossbeam_channel::{Receiver, Sender, bounded};

/// 共享状态结构
/// 设计原则：
/// 1. 终端缓冲区使用 RwLock，读多写少
/// 2. 会话状态使用 Mutex，需要独占访问
/// 3. UI 通过 crossbeam channel 接收事件通知
pub struct SharedState {
    /// 终端缓冲区 - RwLock 允许多个读取者
    pub terminal_buffers: Arc<RwLock<HashMap<SessionId, TerminalBuffer>>>,

    /// 活动会话 - Mutex 保证独占访问
    pub sessions: Arc<Mutex<HashMap<SessionId, ActiveSession>>>,

    /// 传输任务队列
    pub transfer_queue: Arc<Mutex<Vec<TransferTask>>>,

    /// 连接状态（简单数据用原子操作）
    pub connection_status: Arc<std::sync::RwLock<HashMap<SessionId, ConnectionStatus>>>,
}

/// UI 线程持有的状态快照
/// 每帧从 SharedState 拉取更新
pub struct UiState {
    /// 终端缓冲区快照（只读）
    terminal_buffers: HashMap<SessionId, TerminalBuffer>,

    /// 连接状态快照
    connection_status: HashMap<SessionId, ConnectionStatus>,

    /// 待处理事件
    pending_events: Vec<Event>,
}

/// 应用状态管理器
pub struct AppState {
    /// 共享状态（与 tokio 任务共享）
    shared: SharedState,

    /// UI 状态（仅 UI 线程访问）
    ui: UiState,

    /// 事件通道（tokio -> UI）
    event_rx: Receiver<Event>,
    event_tx: Sender<Event>,

    /// 命令通道（UI -> tokio）
    command_tx: mpsc::Sender<Command>,
}

impl AppState {
    /// 在 UI 帧更新时调用
    /// 拉取所有待处理事件并更新 UI 状态
    pub fn update(&mut self) {
        // 1. 非阻塞地拉取所有事件
        while let Ok(event) = self.event_rx.try_recv() {
            self.ui.pending_events.push(event);
        }

        // 2. 处理事件，更新 UI 状态
        for event in self.ui.pending_events.drain(..) {
            match event {
                Event::TerminalOutput(session_id, data) => {
                    // 更新终端缓冲区快照
                    if let Some(buffer) = self.ui.terminal_buffers.get_mut(&session_id) {
                        buffer.process_output(&data);
                    }
                }
                Event::Connected(session_id) => {
                    self.ui.connection_status.insert(session_id, ConnectionStatus::Connected);
                }
                Event::Disconnected(session_id, reason) => {
                    self.ui.connection_status.insert(session_id, ConnectionStatus::Disconnected);
                    // 可选：显示断开原因
                }
                _ => {}
            }
        }

        // 3. 同步连接状态（低频操作）
        if let Ok(status) = self.shared.connection_status.read() {
            self.ui.connection_status = status.clone();
        }
    }

    /// 发送命令到 tokio 运行时
    pub fn send_command(&self, cmd: Command) {
        let _ = self.command_tx.blocking_send(cmd);
    }
}

/// 锁顺序规则（防止死锁）：
/// 1. 永远按照 terminal_buffers -> sessions -> transfer_queue 顺序获取锁
/// 2. 不要在持有锁的情况下调用可能阻塞的操作
/// 3. 尽量减少锁的持有时间
```

**终端缓冲区同步策略**：

```rust
/// 终端缓冲区的增量更新机制
pub struct TerminalBuffer {
    /// 完整内容
    grid: Grid,

    /// 脏区域标记（减少渲染工作量）
    damage: DamageTracker,

    /// 版本号（用于检测变化）
    version: u64,
}

/// 脏区域追踪器
pub struct DamageTracker {
    /// 受影响的行范围
    damaged_rows: Vec<Range<usize>>,

    /// 是否需要完全重绘
    full_redraw: bool,
}

impl TerminalBuffer {
    /// 处理输出，标记脏区域
    pub fn process_output(&mut self, data: &[u8]) {
        let start_row = self.grid.cursor_row();
        // ... 解析并更新 grid ...
        let end_row = self.grid.cursor_row();

        self.damage.damaged_rows.push(start_row..end_row);
        self.version += 1;
    }

    /// 获取脏区域并清除
    pub fn take_damage(&mut self) -> Vec<Range<usize>> {
        std::mem::take(&mut self.damage.damaged_rows)
    }
}
```

**复制粘贴快捷键处理**：

```rust
/// 在 UI 层处理复制粘贴，不发送到 PTY
impl TerminalView {
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        // 复制粘贴在 UI 层处理
        if key.modifiers.ctrl && key.logical_key == Key::Character("c".into()) {
            if self.selection.is_some() {
                self.copy_selection();
                return true;  // 消费事件，不发送到 PTY
            }
        }

        if key.modifiers.ctrl && key.logical_key == Key::Character("v".into()) {
            self.paste();
            return true;
        }

        // 其他按键发送到终端处理
        self.terminal.handle_key_event(key)
    }
}
```

### 7.5 配置文件结构

```
~/.config/rsterm/
├── config.toml              # 应用配置
├── sessions.toml            # 会话配置
├── groups.toml              # 分组配置
├── history.toml             # 命令历史
├── snippets.toml            # 代码片段
├── keybindings.toml         # 快捷键配置
├── themes/
│   ├── dark.toml
│   ├── light.toml
│   └── custom/
│       └── my-theme.toml
└── cache/
    ├── known_hosts          # SSH 已知主机
    └── last_session.json    # 上次会话状态
```

**config.toml 示例**：

```toml
[app]
language = "zh-CN"
theme = "dark"
font = "JetBrains Mono"
font_size = 14
line_height = 1.2
check_update_on_start = false

[terminal]
scroll_buffer = 10000
copy_on_select = false
right_click_paste = true
bell_style = "visual"  # visual / audible / none
cursor_style = "block"
cursor_blink = true

[ssh]
connection_timeout = 30
keep_alive_interval = 30
keep_alive_count_max = 3
compression = true

[sftp]
default_local_path = "~"
default_remote_path = "~"
show_hidden_files = true
confirm_overwrite = true

[update]
channel = "stable"  # stable / beta
check_interval = 86400  # seconds
```

### 7.6 错误处理策略

```rust
/// 应用错误类型
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("SSH connection failed: {0}")]
    SshConnection(#[from] russh::Error),

    #[error("Authentication failed: {0}")]
    Authentication(String),

    #[error("Host key verification failed")]
    HostKeyVerification,

    #[error("Connection timeout")]
    Timeout,

    #[error("Network error: {0}")]
    Network(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(#[from] toml::de::Error),

    #[error("Keyring error: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("SFTP error: {0}")]
    Sftp(String),
}

/// 连接错误恢复策略
pub enum RecoveryStrategy {
    Retry { max_attempts: u32, delay: Duration },
    PromptUser,
    GiveUp,
}
```

---

## 八、关键技术决策

### 8.1 为什么选择 egui 而不是 Tauri？

| 考量 | egui | Tauri |
|-----|------|-------|
| **性能** | ✅ 原生渲染，极低开销 | 需要 WebView 运行时 |
| **内存** | ✅ < 50MB | 100-200MB (WebView) |
| **启动速度** | ✅ 即时 | 需要加载 WebView |
| **开发体验** | 纯 Rust，即时模式 | 需要 HTML/CSS/JS |
| **体积** | ✅ 单文件 < 10MB | 安装包较大 |
| **跨平台** | ✅ 一致体验 | 平台 WebView 差异 |

**结论**：对于终端应用，egui 的轻量级特性更适合，且纯 Rust 技术栈更统一。

### 8.2 为什么选择 russh 而不是 ssh2-rs？

| 考量 | russh | ssh2-rs |
|-----|-------|---------|
| **实现** | 纯 Rust | libssh2 绑定 |
| **安全性** | ✅ 内存安全 | 依赖 C 库 |
| **异步支持** | ✅ 原生 tokio | 需要封装 |
| **维护状态** | ✅ 活跃 | 较少更新 |
| **功能完整性** | 完整 | 完整 |

**结论**：russh 是纯 Rust 实现，与 tokio 集成良好，安全性更高。

### 8.3 终端渲染方案

**方案对比**：

1. **自研渲染（基于 vte）** — 灵活可控，但工作量大
2. **集成 alacritty_terminal** — 成熟方案，但依赖较重
3. **集成 wezterm 模块** — 功能强大，但复杂度高

**推荐**：自研轻量渲染，基于 vte 解析器 + egui 绘制。终端模拟核心逻辑相对固定，自研可以更好控制性能和定制。

---

## 九、核心子系统深入设计

### 9.1 SSH 子系统设计

#### 9.1.1 模块架构

```
ssh/
├── mod.rs              # 模块入口
├── client.rs           # SSH 客户端封装
├── auth.rs             # 认证处理
├── session.rs          # 会话管理
├── channel.rs          # 通道管理
├── tunnel.rs           # 端口转发
├── handler.rs          # 事件处理器
└── error.rs            # 错误定义
```

#### 9.1.2 核心接口设计

```rust
use std::sync::Arc;
use russh::*;
use russh::keys::*;
use tokio::sync::{mpsc, Mutex};

/// SSH 客户端配置
pub struct SshClientConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub proxy: Option<ProxyConfig>,
    pub jump_hosts: Vec<JumpHostConfig>,
    pub timeout: Duration,
    pub keep_alive: KeepAliveConfig,
}

/// SSH 客户端
pub struct SshClient {
    config: SshClientConfig,
    session: Option<client::Handle<ClientHandler>>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel>>>,
    event_tx: mpsc::Sender<SshEvent>,
}

/// 客户端处理器
pub struct ClientHandler {
    event_tx: mpsc::Sender<SshEvent>,
}

impl client::Handler for ClientHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // 发送事件让 UI 决定是否信任
        let (tx, rx) = oneshot::channel();
        self.event_tx.send(SshEvent::ServerKeyVerify {
            public_key: server_public_key.clone(),
            response: tx,
        }).await?;
        Ok(rx.await?)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // 转发终端输出
        self.event_tx.send(SshEvent::TerminalOutput {
            channel,
            data: data.to_vec(),
        }).await?;
        Ok(())
    }
}

/// SSH 事件
pub enum SshEvent {
    Connected,
    Disconnected { reason: Option<String> },
    ServerKeyVerify {
        public_key: ssh_key::PublicKey,
        response: oneshot::Sender<bool>,
    },
    TerminalOutput {
        channel: ChannelId,
        data: Vec<u8>,
    },
    ChannelClosed { channel: ChannelId },
    Error(String),
}

/// SSH 服务接口
impl SshClient {
    /// 建立连接
    pub async fn connect(&mut self) -> Result<(), SshError> {
        // 1. 处理跳板机链
        let mut current_stream = None;
        for jump in &self.config.jump_hosts {
            current_stream = Some(
                self.connect_through_jump(
                    current_stream.as_ref(),
                    jump
                ).await?
            );
        }

        // 2. 处理代理
        let stream = if let Some(ref proxy) = self.config.proxy {
            self.connect_via_proxy(current_stream, proxy).await?
        } else if let Some(stream) = current_stream {
            stream
        } else {
            tokio::net::TcpStream::connect(
                (&self.config.host[..], self.config.port)
            ).await?
        };

        // 3. 建立 SSH 会话
        let config = Arc::new(client::Config::default());
        let mut session = client::connect(
            config,
            None,
            stream,
            ClientHandler {
                event_tx: self.event_tx.clone(),
            },
        ).await?;

        // 4. 认证
        self.authenticate(&mut session).await?;

        self.session = Some(session);
        self.event_tx.send(SshEvent::Connected).await?;
        Ok(())
    }

    /// 打开 PTY 通道
    pub async fn open_pty(
        &mut self,
        cols: u16,
        rows: u16,
        term: &str,
    ) -> Result<ChannelId, SshError> {
        let session = self.session.as_ref().ok_or(SshError::NotConnected)?;
        let channel = session.channel_open_session().await?;
        channel.request_pty(
            false,
            term,
            cols,
            rows,
            0,
            0,
            &[],
        ).await?;
        channel.request_shell(true).await?;

        let id = channel.id();
        self.channels.lock().await.insert(id, channel);
        Ok(id)
    }

    /// 调整终端大小
    pub async fn resize_pty(
        &self,
        channel: ChannelId,
        cols: u16,
        rows: u16,
    ) -> Result<(), SshError> {
        let channels = self.channels.lock().await;
        let ch = channels.get(&channel).ok_or(SshError::InvalidChannel)?;
        ch.resize_pty(cols, rows, 0, 0).await?;
        Ok(())
    }

    /// 发送数据
    pub async fn send_data(
        &self,
        channel: ChannelId,
        data: &[u8],
    ) -> Result<(), SshError> {
        let channels = self.channels.lock().await;
        let ch = channels.get(&channel).ok_or(SshError::InvalidChannel)?;
        ch.data(data).await?;
        Ok(())
    }

    /// 断开连接
    pub async fn disconnect(&mut self, reason: &str) -> Result<(), SshError> {
        if let Some(session) = self.session.take() {
            session.disconnect(None, reason, "").await?;
        }
        self.channels.lock().await.clear();
        Ok(())
    }
}
```

#### 9.1.3 认证流程

```
┌─────────────────────────────────────────────────────────────┐
│                     Authentication Flow                      │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────────┐                                           │
│  │ Start Connect│                                           │
│  └──────┬───────┘                                           │
│         │                                                    │
│         ▼                                                    │
│  ┌──────────────┐                                           │
│  │ Get Auth     │                                           │
│  │ Config       │                                           │
│  └──────┬───────┘                                           │
│         │                                                    │
│         ▼                                                    │
│  ┌──────────────────────────────────────┐                  │
│  │         Auth Type?                    │                  │
│  └──────┬───────┬───────┬───────┬───────┘                  │
│         │       │       │       │                           │
│    Password   Key    Cert   Agent                          │
│         │       │       │       │                           │
│         ▼       ▼       ▼       ▼                           │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐      │
│  │Keyring   │ │Load Key  │ │Load Cert │ │Connect   │      │
│  │Get Pwd   │ │+ Passphr │ │+ Key     │ │Agent     │      │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘      │
│       │            │            │            │              │
│       ▼            ▼            ▼            ▼              │
│  ┌────────────────────────────────────────────────────┐    │
│  │           authenticate_password/key/publickey       │    │
│  └───────────────────────┬────────────────────────────┘    │
│                          │                                  │
│                          ▼                                  │
│                   ┌─────────────┐                          │
│                   │  Success?   │                          │
│                   └──────┬──────┘                          │
│                     Yes  │  No                              │
│                     │    └─────────▶ Retry / Error         │
│                     ▼                                        │
│              ┌─────────────┐                                │
│              │ Connected   │                                │
│              └─────────────┘                                │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

#### 9.1.4 跳板机连接流程

```rust
impl SshClient {
    async fn connect_through_jump(
        &self,
        existing_stream: Option<&TcpStream>,
        jump: &JumpHostConfig,
    ) -> Result<TcpStream, SshError> {
        // 1. 连接到跳板机
        let stream = if let Some(stream) = existing_stream {
            // 通过现有通道
            self.tunnel_to_next(stream, &jump.host, jump.port).await?
        } else {
            // 直接连接
            TcpStream::connect((&jump.host[..], jump.port)).await?
        };

        // 2. 建立 SSH 会话
        let config = Arc::new(client::Config::default());
        let mut session = client::connect(
            config,
            None,
            stream,
            ClientHandler {
                event_tx: self.event_tx.clone(),
            },
        ).await?;

        // 3. 认证
        self.authenticate_jump(&mut session, jump).await?;

        // 4. 开启端口转发通道到下一跳
        let target_stream = self.port_forward(&session, &jump.target_host, jump.target_port).await?;

        Ok(target_stream)
    }
}
```

### 9.2 Terminal 子系统设计

#### 9.2.1 模块架构

```
terminal/
├── mod.rs              # 模块入口
├── pty.rs              # PTY 抽象层
├── parser.rs           # VT100 解析器 (vte 封装)
├── buffer.rs           # 滚动缓冲
├── grid.rs             # 屏幕网格
├── cursor.rs           # 光标管理
├── cell.rs             # 单元格
├── selection.rs        # 文本选择
├── search.rs           # 搜索功能
└── render.rs           # 渲染接口
```

#### 9.2.2 核心数据结构

```rust
use vte::{Parser, Perform};

/// 终端实例
pub struct Terminal {
    // 屏幕状态
    grid: Grid,
    alternate_grid: Option<Grid>,
    cursor: Cursor,
    saved_cursor: Option<Cursor>,

    // 滚动缓冲
    scrollback: ScrollBuffer,
    scroll_offset: usize,

    // 解析器
    parser: Parser,
    performer: TerminalPerformer,

    // PTY 后端
    pty: Box<dyn PtyBackend>,

    // 尺寸
    size: Size,

    // 选择/搜索
    selection: Option<Selection>,
    search: Option<SearchState>,

    // 渲染状态
    damage: DamageTracker,
}

/// 屏幕网格
pub struct Grid {
    rows: Vec<Row>,
    size: Size,
}

/// 行
pub struct Row {
    cells: Vec<Cell>,
    wrapped: bool,
}

/// 单元格
#[derive(Clone, Debug)]
pub struct Cell {
    pub char: char,
    pub width: u8,           // 字符宽度 (1 或 2，用于 CJK)
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

bitflags! {
    pub struct CellFlags: u8 {
        const BOLD = 0x01;
        const DIM = 0x02;
        const ITALIC = 0x04;
        const UNDERLINE = 0x08;
        const BLINK = 0x10;
        const REVERSE = 0x20;
        const HIDDEN = 0x40;
        const STRIKETHROUGH = 0x80;
    }
}

/// 光标
#[derive(Clone, Debug)]
pub struct Cursor {
    pub position: Position,
    pub style: CursorStyle,
    pub visible: bool,
    pub pending_wrap: bool,
    pub save_point: Option<Position>,
}

#[derive(Clone, Copy, Debug)]
pub enum CursorStyle {
    Block,
    Underline,
    Bar,
}

/// 滚动缓冲
pub struct ScrollBuffer {
    lines: VecDeque<Row>,
    max_lines: usize,
}

/// 终端尺寸
#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

/// 文本选择
#[derive(Clone, Debug)]
pub struct Selection {
    pub start: Position,
    pub end: Position,
    pub mode: SelectionMode,
}

#[derive(Clone, Copy, Debug)]
pub enum SelectionMode {
    Character,
    Word,
    Line,
    Block,
}

/// 搜索状态
pub struct SearchState {
    pub query: String,
    pub regex: Option<Regex>,
    pub matches: Vec<Range<Position>>,
    pub current_match: usize,
    pub direction: SearchDirection,
}

/// 终端字体配置
pub struct TerminalFonts {
    /// 主字体（常规）
    pub regular: egui::FontId,
    /// 粗体字体
    pub bold: egui::FontId,
    /// 斜体字体
    pub italic: egui::FontId,
    /// 粗斜体字体
    pub bold_italic: egui::FontId,
    /// 单元格尺寸 (width, height)
    pub cell_size: egui::Vec2,
}

impl TerminalFonts {
    pub fn new(font_name: &str, font_size: f32) -> Self {
        Self {
            regular: egui::FontId::proportional(font_size),
            bold: egui::FontId::proportional(font_size),
            italic: egui::FontId::proportional(font_size),
            bold_italic: egui::FontId::proportional(font_size),
            // 需要根据实际字体测量
            cell_size: egui::Vec2::new(font_size * 0.6, font_size * 1.2),
        }
    }

    pub fn get_font(&self, flags: CellFlags) -> &egui::FontId {
        match (flags.contains(CellFlags::BOLD), flags.contains(CellFlags::ITALIC)) {
            (true, true) => &self.bold_italic,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (false, false) => &self.regular,
        }
    }
}

/// 脏区域追踪器
pub struct DamageTracker {
    /// 受影响的行范围
    pub damaged_rows: Vec<Range<usize>>,
    /// 是否需要完全重绘
    pub full_redraw: bool,
}

impl DamageTracker {
    pub fn new() -> Self {
        Self {
            damaged_rows: Vec::new(),
            full_redraw: false,
        }
    }

    /// 标记行范围为脏
    pub fn mark_dirty(&mut self, rows: Range<usize>) {
        self.damaged_rows.push(rows);
    }

    /// 标记需要完全重绘
    pub fn mark_full_redraw(&mut self) {
        self.full_redraw = true;
        self.damaged_rows.clear();
    }

    /// 获取脏区域并清除
    pub fn take_damage(&mut self) -> Vec<Range<usize>> {
        std::mem::take(&mut self.damaged_rows)
    }

    /// 是否有任何损坏
    pub fn is_dirty(&self) -> bool {
        self.full_redraw || !self.damaged_rows.is_empty()
    }
}

/// 搜索方向
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// 位置（行列）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Position {
    pub row: usize,
    pub col: usize,
}
```

#### 9.2.3 VT100 解析处理

```rust
/// 终端转义序列处理器
struct TerminalPerformer<'a> {
    terminal: &'a mut Terminal,
}

impl Perform for TerminalPerformer<'_> {
    /// 打印字符
    fn print(&mut self, c: char) {
        self.terminal.put_char(c);
    }

    /// 执行控制字符 (LF, CR, TAB, BS 等)
    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => self.terminal.backspace(),      // BS
            0x09 => self.terminal.tab(),            // TAB
            0x0A => self.terminal.linefeed(),       // LF
            0x0D => self.terminal.carriage_return(), // CR
            0x07 => self.terminal.bell(),           // BEL
            _ => {}
        }
    }

    /// CSI 序列 (如 \x1b[...m 颜色)
    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        match action {
            'A' => self.terminal.cursor_up(params),      // CUU
            'B' => self.terminal.cursor_down(params),    // CUD
            'C' => self.terminal.cursor_forward(params), // CUF
            'D' => self.terminal.cursor_back(params),    // CUB
            'H' => self.terminal.cursor_position(params),// CUP
            'J' => self.terminal.erase_display(params),  // ED
            'K' => self.terminal.erase_line(params),     // EL
            'm' => self.terminal.sgr(params),            // SGR
            'r' => self.terminal.set_scrolling_region(params),
            's' => self.terminal.save_cursor(),
            'u' => self.terminal.restore_cursor(),
            '?' => self.terminal.csi_private(params, intermediates),
            _ => {}
        }
    }

    /// OSC 序列 (如标题设置)
    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        if params.is_empty() {
            return;
        }

        match params[0] {
            b"0" | b"2" => {
                // 设置窗口标题
                if params.len() > 1 {
                    let title = String::from_utf8_lossy(params[1]);
                    self.terminal.set_title(&title);
                }
            }
            b"7" => {
                // 设置当前工作目录
                if params.len() > 1 {
                    self.terminal.set_cwd(params[1]);
                }
            }
            b"8" => {
                // Hyperlink
                if params.len() > 2 {
                    self.terminal.set_hyperlink(params[1], params[2]);
                }
            }
            _ => {}
        }
    }
}
```

#### 9.2.4 渲染流程

```rust
impl Terminal {
    /// 渲染到 egui
    pub fn render(&mut self, ui: &mut egui::Ui, fonts: &TerminalFonts) {
        let available = ui.available_size();
        let cell_size = fonts.cell_size;

        // 计算可见区域
        let visible_rows = (available.y / cell_size.y).floor() as usize;
        let visible_cols = (available.x / cell_size.x).floor() as usize;

        // 获取绘制区域
        let painter = ui.painter();

        // 绘制背景
        painter.rect_filled(
            ui.max_rect(),
            0.0,
            self.theme.colors.background,
        );

        // 绘制可见行
        for row_idx in 0..visible_rows.min(self.grid.rows.len()) {
            let row = &self.grid.rows[row_idx];
            let y = row_idx as f32 * cell_size.y;

            for (col_idx, cell) in row.cells.iter().enumerate() {
                let x = col_idx as f32 * cell_size.x;

                // 绘制背景色
                if cell.bg != self.theme.colors.background {
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(x, y),
                            cell_size,
                        ),
                        0.0,
                        cell.bg,
                    );
                }

                // 绘制字符
                if cell.char != ' ' && cell.char != '\0' {
                    let color = if cell.flags.contains(CellFlags::REVERSE) {
                        cell.bg
                    } else {
                        cell.fg
                    };

                    painter.text(
                        egui::pos2(x, y),
                        egui::Align2::LEFT_TOP,
                        cell.char.to_string(),
                        fonts.get_font(cell.flags),
                        color,
                    );
                }
            }
        }

        // 绘制光标
        if self.cursor.visible {
            self.render_cursor(&painter, cell_size);
        }

        // 绘制选择高亮
        if let Some(ref selection) = self.selection {
            self.render_selection(&painter, selection, cell_size);
        }

        // 绘制搜索高亮
        if let Some(ref search) = self.search {
            self.render_search_matches(&painter, search, cell_size);
        }
    }
}
```

#### 9.2.5 输入处理

```rust
impl Terminal {
    /// 处理键盘输入
    pub fn handle_key_event(&mut self, event: &KeyEvent) -> bool {
        let mods = event.modifiers;

        match event.logical_key {
            // 功能键
            Key::Enter => self.send_key("\r"),
            Key::Backspace => self.send_key("\x7f"),
            Key::Tab => {
                if mods.shift {
                    self.send_key("\x1b[Z")  // Shift+Tab
                } else {
                    self.send_key("\t")
                }
            }
            Key::Escape => self.send_key("\x1b"),

            // 方向键
            Key::ArrowUp => self.send_csi_cursor('A', mods),
            Key::ArrowDown => self.send_csi_cursor('B', mods),
            Key::ArrowRight => self.send_csi_cursor('C', mods),
            Key::ArrowLeft => self.send_csi_cursor('D', mods),

            // 功能键 F1-F12
            Key::F(n) => self.send_function_key(n, mods),

            // Home/End
            Key::Home => self.send_key("\x1b[H"),
            Key::End => self.send_key("\x1b[F"),

            // Page Up/Down
            Key::PageUp => self.send_key("\x1b[5~"),
            Key::PageDown => self.send_key("\x1b[6~"),

            // 普通字符
            Key::Character(ch) => {
                if mods.ctrl && ch.len() == 1 {
                    // Ctrl 组合键
                    let c = ch.chars().next().unwrap();
                    if c.is_ascii_lowercase() {
                        let code = (c as u8) - b'a' + 1;
                        self.send_byte(code)
                    } else {
                        false
                    }
                } else {
                    self.send_key(&ch)
                }
            }

            _ => false,
        }
    }

    /// 发送 CSI 序列光标键
    fn send_csi_cursor(&mut self, dir: char, mods: Modifiers) -> bool {
        let seq = if mods.any() {
            // 应用修饰符
            let mut code = 1;
            if mods.shift { code += 1; }
            if mods.alt { code += 2; }
            if mods.ctrl { code += 4; }
            format!("\x1b[1;{}{}", code, dir)
        } else {
            format!("\x1b[{}", dir)
        };
        self.send_key(&seq)
    }
}
```

### 9.3 SFTP 子系统设计

#### 9.3.1 模块架构

```
sftp/
├── mod.rs              # 模块入口
├── client.rs           # SFTP 客户端
├── file_info.rs        # 文件信息
├── transfer.rs         # 传输管理
├── browser.rs          # 文件浏览
└── error.rs            # 错误定义
```

#### 9.3.2 核心接口

```rust
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{OpenFlags, File, Attributes};

/// SFTP 客户端封装
pub struct SftpClient {
    session: SftpSession,
    current_path: PathBuf,
}

impl SftpClient {
    /// 从 SSH 会话创建 SFTP 客户端
    pub async fn from_ssh_session(
        ssh_session: &client::Handle<impl client::Handler>,
    ) -> Result<Self, SftpError> {
        let channel = ssh_session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let sftp = SftpSession::new(channel.into_stream()).await?;
        let cwd = sftp.canonicalize(".").await?;

        Ok(Self {
            session: sftp,
            current_path: cwd,
        })
    }

    /// 列出目录内容
    pub async fn list_dir(&self, path: &Path) -> Result<Vec<FileInfo>, SftpError> {
        let entries = self.session.read_dir(path).await?;
        Ok(entries.into_iter().map(FileInfo::from).collect())
    }

    /// 下载文件
    pub async fn download(
        &self,
        remote_path: &Path,
        local_path: &Path,
        progress: Option<ProgressCallback>,
    ) -> Result<(), SftpError> {
        let mut remote_file = self.session.open(remote_path).await?;
        let mut local_file = tokio::fs::File::create(local_path).await?;

        // 获取文件大小
        let metadata = self.session.metadata(remote_path).await?;
        let total_size = metadata.len();

        let mut downloaded = 0u64;
        let mut buffer = vec![0u8; 64 * 1024]; // 64KB 缓冲

        loop {
            let n = remote_file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }

            local_file.write_all(&buffer[..n]).await?;
            downloaded += n as u64;

            if let Some(ref cb) = progress {
                cb(downloaded, total_size);
            }
        }

        Ok(())
    }

    /// 上传文件
    pub async fn upload(
        &self,
        local_path: &Path,
        remote_path: &Path,
        progress: Option<ProgressCallback>,
    ) -> Result<(), SftpError> {
        let mut local_file = tokio::fs::File::open(local_path).await?;
        let metadata = local_file.metadata().await?;
        let total_size = metadata.len();

        let mut remote_file = self.session
            .open_with_flags(
                remote_path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await?;

        let mut uploaded = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];

        loop {
            let n = local_file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }

            remote_file.write_all(&buffer[..n]).await?;
            uploaded += n as u64;

            if let Some(ref cb) = progress {
                cb(uploaded, total_size);
            }
        }

        remote_file.flush().await?;
        Ok(())
    }

    /// 创建目录
    pub async fn mkdir(&self, path: &Path) -> Result<(), SftpError> {
        self.session.create_dir(path).await?;
        Ok(())
    }

    /// 删除文件
    pub async fn remove_file(&self, path: &Path) -> Result<(), SftpError> {
        self.session.remove_file(path).await?;
        Ok(())
    }

    /// 删除目录
    pub async fn remove_dir(&self, path: &Path) -> Result<(), SftpError> {
        self.session.remove_dir(path).await?;
        Ok(())
    }

    /// 重命名
    pub async fn rename(&self, from: &Path, to: &Path) -> Result<(), SftpError> {
        self.session.rename(from, to).await?;
        Ok(())
    }

    /// 修改权限
    pub async fn chmod(&self, path: &Path, mode: u32) -> Result<(), SftpError> {
        self.session.set_metadata(
            path,
            Attributes {
                permissions: Some(mode),
                ..Default::default()
            },
        ).await?;
        Ok(())
    }
}

/// 文件信息
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    pub permissions: u32,
}

/// 进度回调
pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send + Sync>;
```

#### 9.3.3 传输队列管理

```rust
use tokio::sync::mpsc;

/// 传输任务
#[derive(Debug, Clone)]
pub struct TransferTask {
    pub id: TransferId,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub direction: TransferDirection,
    pub state: TransferState,
    pub progress: f32,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransferState {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

/// 传输管理器
pub struct TransferManager {
    tasks: Arc<Mutex<Vec<TransferTask>>>,
    command_tx: mpsc::Sender<TransferCommand>,
    event_tx: mpsc::Sender<TransferEvent>,
}

/// 传输命令
pub enum TransferCommand {
    Add(TransferTask),
    Start(TransferId),
    Pause(TransferId),
    Cancel(TransferId),
    Retry(TransferId),
    ClearCompleted,
}

/// 传输事件
pub enum TransferEvent {
    Progress { id: TransferId, progress: f32 },
    Completed { id: TransferId },
    Failed { id: TransferId, error: String },
}

impl TransferManager {
    pub fn new(sftp: Arc<SftpClient>) -> Self {
        let (command_tx, command_rx) = mpsc::channel(100);
        let (event_tx, event_rx) = mpsc::channel(100);
        let tasks = Arc::new(Mutex::new(Vec::new()));

        // 启动传输工作线程
        tokio::spawn(Self::worker(
            sftp,
            tasks.clone(),
            command_rx,
            event_tx.clone(),
        ));

        Self {
            tasks,
            command_tx,
            event_tx,
        }
    }

    async fn worker(
        sftp: Arc<SftpClient>,
        tasks: Arc<Mutex<Vec<TransferTask>>>,
        mut command_rx: mpsc::Receiver<TransferCommand>,
        event_tx: mpsc::Sender<TransferEvent>,
    ) {
        while let Some(cmd) = command_rx.recv().await {
            match cmd {
                TransferCommand::Add(mut task) => {
                    task.state = TransferState::Pending;
                    tasks.lock().await.push(task);
                }
                TransferCommand::Start(id) => {
                    // 执行传输
                    let task = {
                        let tasks = tasks.lock().await;
                        tasks.iter().find(|t| t.id == id).cloned()
                    };

                    if let Some(task) = task {
                        let sftp = sftp.clone();
                        let event_tx = event_tx.clone();
                        let tasks_clone = tasks.clone();

                        tokio::spawn(async move {
                            // 更新状态为运行中
                            tasks_clone.lock().await.iter_mut()
                                .find(|t| t.id == id)
                                .map(|t| t.state = TransferState::Running);

                            let result = match task.direction {
                                TransferDirection::Download => {
                                    sftp.download(
                                        &task.source,
                                        &task.destination,
                                        Some(Box::new(|current, total| {
                                            let progress = if total > 0 {
                                                current as f32 / total as f32
                                            } else {
                                                0.0
                                            };
                                            // 发送进度事件
                                        })),
                                    ).await
                                }
                                TransferDirection::Upload => {
                                    sftp.upload(
                                        &task.source,
                                        &task.destination,
                                        Some(Box::new(|current, total| {
                                            let progress = if total > 0 {
                                                current as f32 / total as f32
                                            } else {
                                                0.0
                                            };
                                        })),
                                    ).await
                                }
                            };

                            match result {
                                Ok(()) => {
                                    tasks_clone.lock().await.iter_mut()
                                        .find(|t| t.id == id)
                                        .map(|t| {
                                            t.state = TransferState::Completed;
                                            t.progress = 1.0;
                                        });
                                    event_tx.send(TransferEvent::Completed { id }).await;
                                }
                                Err(e) => {
                                    tasks_clone.lock().await.iter_mut()
                                        .find(|t| t.id == id)
                                        .map(|t| {
                                            t.state = TransferState::Failed;
                                            t.error = Some(e.to_string());
                                        });
                                    event_tx.send(TransferEvent::Failed {
                                        id,
                                        error: e.to_string(),
                                    }).await;
                                }
                            }
                        });
                    }
                }
                _ => {}
            }
        }
    }
}
```

### 9.4 本地 PTY 支持

```rust
use portable_pty::{native_pty_system, PtySize, CommandBuilder};

/// 本地 PTY 后端
pub struct LocalPty {
    pair: Box<dyn PtyPair>,
    reader: Box<dyn Read>,
    writer: Box<dyn Write>,
}

impl LocalPty {
    pub fn new(
        shell: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // 启动 shell
        let mut cmd = CommandBuilder::new(shell);
        let _child = pair.slave.spawn_command(cmd)?;

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        Ok(Self {
            pair,
            reader,
            writer,
        })
    }
}

impl PtyBackend for LocalPty {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.reader.read(buf).map_err(Into::into)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.pair.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    fn is_alive(&self) -> bool {
        // 检查子进程是否还在运行
        true
    }
}
```

---

## 十、实现路线图

### Phase 1: 基础框架 (v0.1.0)

1. **项目初始化**
   - Cargo.toml 依赖配置
   - 基础模块结构搭建
   - 日志系统配置

2. **SSH 连接**
   - russh 集成
   - 密码/公钥认证
   - 会话配置持久化

3. **终端基础**
   - VT100 解析器 (vte)
   - 基础屏幕渲染
   - 键盘输入处理

4. **UI 框架**
   - egui 应用框架
   - 多标签页
   - 基础布局

### Phase 2: 功能完善 (v0.2.0)

5. **终端增强**
   - 滚动缓冲
   - 文本选择/复制
   - 搜索功能
   - URL 识别

6. **会话管理**
   - 分组管理
   - 搜索过滤
   - 导入/导出

7. **高级连接**
   - 跳板机支持
   - 代理支持
   - 断线重连

### Phase 3: SFTP 集成 (v0.3.0)

8. **文件浏览**
   - 双面板界面
   - 文件列表
   - 目录导航

9. **文件传输**
   - 拖拽上传/下载
   - 传输队列
   - 进度显示

### Phase 4: 效率工具 (v0.4.0)

10. **命令管理**
    - 命令历史
    - 代码片段
    - 快捷命令

11. **分屏与同步**
    - 水平/垂直分屏
    - 同步输入

12. **端口转发**
    - 本地转发
    - 远程转发
    - 图形化管理

---

*文档完成。下一步：开始实现计划编写。*