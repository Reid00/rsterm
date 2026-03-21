# RSTerm v0.1.0 MVP 详细开发规格书

> 创建日期: 2026-03-21
> 基于: rsterm-feature-design.md
> 目标: 细化 MVP 所有子系统到可直接编码的程度

---

## 一、MVP 范围定义

### 1.1 包含功能

| 模块 | 功能 | 说明 |
|------|------|------|
| SSH | SSH2 连接、密码/公钥认证、心跳保活、断线重连 | 单次连接，不含跳板机/代理 |
| 终端 | PTY 分配、xterm-256color、UTF-8/CJK、光标样式、滚动缓冲、基础选择复制 | 不含搜索、URL识别 |
| 本地终端 | 本地 shell PTY | Windows: PowerShell/cmd, macOS/Linux: 默认 shell |
| SFTP | 列目录、单文件上传/下载、mkdir/delete/rename | 不含拖拽、传输队列UI、收藏夹 |
| UI | 单窗口多标签、会话面板(扁平列表)、状态栏、深色/浅色主题、新建连接对话框、设置对话框 | 不含分组树、分屏、搜索面板 |
| 配置 | TOML 会话保存/加载、应用设置持久化 | 不含加密存储（密码明文存TOML或用系统keyring） |
| 日志 | tracing 基础日志 | 文件+控制台输出 |

### 1.2 明确排除（推迟到后续版本）

- 跳板机、SOCKS5/HTTP 代理
- 分屏、同步输入
- 会话分组树、标签、搜索过滤
- 终端搜索、正则搜索、URL/文件路径识别
- SFTP 拖拽传输、传输队列 UI、权限修改
- 端口转发
- 命令历史、代码片段、自动补全
- 加密密码存储（keyring 仅作为可选项）
- 自定义快捷键绑定
- 多语言（首版仅英文 UI）

---

## 二、模块接口规格

### 2.1 PtyBackend trait — 异步 push 模式

现有 spec 用同步 `Read/Write`，但 SSH 通道是异步的。统一为异步接口，且去掉 `read()` 改用 push 模式——后端通过 `output_tx: mpsc::Sender<Vec<u8>>` 主动推送数据。

```rust
#[async_trait]
pub trait PtyBackend: Send + Sync + 'static {
    async fn write(&self, data: &[u8]) -> Result<(), PtyError>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError>;
    fn is_alive(&self) -> bool;
    // 注意：没有 read()
    // 后端通过 output_tx: mpsc::Sender<Vec<u8>> 主动推送数据
    // SSH 通道的 data 回调天然是 push 的
    // 本地 PTY 也 spawn 一个读取循环 push 到 channel
    // Terminal 不需要轮询
}
```

### 2.2 Terminal 模块 — 解耦输入输出

Terminal 不拥有 PtyBackend，只处理纯终端逻辑：

```rust
pub struct Terminal {
    // 构造时传入，Terminal 不拥有 PtyBackend
    output_tx: mpsc::Sender<Vec<u8>>,  // 发送到 PTY
    // ... grid, cursor, scrollback 等
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Self;

    /// 处理来自 PTY 的原始输出字节（由外部调用）
    pub fn process_output(&mut self, data: &[u8]);

    /// 处理键盘输入，返回需要发送到 PTY 的字节
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Vec<u8>>;

    /// 获取渲染快照（不可变引用）
    pub fn render_snapshot(&self) -> &Grid;

    /// 获取并清除脏区域
    pub fn take_damage(&mut self) -> DamageInfo;

    /// 调整大小
    pub fn resize(&mut self, cols: u16, rows: u16);

    /// 选择操作
    pub fn start_selection(&mut self, pos: Position, mode: SelectionMode);
    pub fn update_selection(&mut self, pos: Position);
    pub fn get_selection_text(&self) -> Option<String>;
    pub fn clear_selection(&mut self);

    /// 滚动
    pub fn scroll(&mut self, delta: i32);
    pub fn scroll_to_bottom(&mut self);
}
```

### 2.3 SshService — MVP 子集

```rust
pub struct SshService {
    event_tx: mpsc::Sender<AppEvent>,
}

impl SshService {
    pub fn new(event_tx: mpsc::Sender<AppEvent>) -> Self;

    /// 建立连接并打开 PTY，返回数据通道
    pub async fn connect(
        &self,
        config: &SessionConfig,
        terminal_size: Size,
    ) -> Result<SshSession, SshError>;
}

/// SshSession 持有连接生命周期
pub struct SshSession {
    /// 发送数据到远端
    pub input_tx: mpsc::Sender<Vec<u8>>,
    /// 接收远端输出（Terminal.process_output 的数据源）
    pub output_rx: mpsc::Receiver<Vec<u8>>,
    /// 内部句柄用于 resize/disconnect
    handle: SshSessionHandle,
}

impl SshSession {
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    pub async fn disconnect(&self) -> Result<()>;
    pub fn is_alive(&self) -> bool;
}
```

### 2.4 Config 模块

```rust
pub struct ConfigManager {
    config_dir: PathBuf,  // ~/.config/rsterm/
}

impl ConfigManager {
    pub fn new() -> Result<Self, ConfigError>;  // 自动检测平台路径

    // 应用设置
    pub fn load_settings(&self) -> Result<AppSettings, ConfigError>;
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), ConfigError>;

    // 会话管理
    pub fn load_sessions(&self) -> Result<Vec<SessionConfig>, ConfigError>;
    pub fn save_session(&self, session: &SessionConfig) -> Result<(), ConfigError>;
    pub fn delete_session(&self, id: &str) -> Result<(), ConfigError>;

    // 已知主机
    pub fn check_host_key(&self, host: &str, port: u16, fingerprint: &str) -> HostKeyStatus;
    pub fn trust_host_key(&self, host: &str, port: u16, fingerprint: &str) -> Result<(), ConfigError>;

    // 主题
    pub fn load_theme(&self, name: &str) -> Result<Theme, ConfigError>;
    pub fn list_themes(&self) -> Result<Vec<String>, ConfigError>;
}
```

---

## 三、异步架构详细设计

### 3.1 线程模型

```
┌─────────────────────────────────────────────────────────────────┐
│                        进程                                      │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  主线程 (UI Thread)                                       │   │
│  │  - eframe 渲染循环 (~60fps)                              │   │
│  │  - 处理用户输入                                           │   │
│  │  - 非阻塞 try_recv 拉取事件                              │   │
│  │  - 持有 Terminal 实例（直接修改，无锁）                    │   │
│  └──────────────────────────────────────────────────────────┘   │
│         │ command_tx          ▲ event_rx                         │
│         ▼                     │                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Tokio Runtime (独立线程池，4 worker threads)             │   │
│  │                                                           │   │
│  │  ┌─────────────┐  每个活跃连接 spawn:                     │   │
│  │  │ SSH Task    │  - ssh_read_loop: 读SSH -> output_tx     │   │
│  │  │ (per conn)  │  - ssh_write_loop: input_rx -> 写SSH     │   │
│  │  └─────────────┘  - keepalive_loop: 定时心跳              │   │
│  │                                                           │   │
│  │  ┌─────────────┐  每个本地终端 spawn:                     │   │
│  │  │ PTY Task    │  - pty_read_loop: 读PTY -> output_tx     │   │
│  │  │ (per local) │  - pty_write_loop: input_rx -> 写PTY     │   │
│  │  └─────────────┘                                          │   │
│  │                                                           │   │
│  │  ┌─────────────┐  按需 spawn:                             │   │
│  │  │ SFTP Task   │  - 单次文件操作 (list/upload/download)   │   │
│  │  └─────────────┘                                          │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 通道拓扑（完整映射）

```rust
/// 所有跨线程通道的集中定义
pub struct AppChannels {
    // ===== UI -> Backend =====

    /// UI 发出的全局命令（连接/断开/SFTP 操作等）
    pub command_tx: mpsc::Sender<AppCommand>,
    pub command_rx: mpsc::Receiver<AppCommand>,

    // ===== Backend -> UI =====

    /// 后端发出的全局事件（连接成功/失败/SFTP结果等）
    pub event_tx: mpsc::Sender<AppEvent>,
    pub event_rx: mpsc::Receiver<AppEvent>,

    // ===== Per-Session 通道（动态创建） =====
    // 每个会话连接成功后创建：
    //   input_tx/input_rx:  UI键盘输入 -> PTY/SSH 写入
    //   output_tx/output_rx: PTY/SSH 读取 -> UI终端渲染
}

/// 单个会话的数据通道
pub struct SessionChannels {
    pub session_id: SessionId,
    /// UI -> PTY/SSH（键盘输入）
    pub input_tx: mpsc::Sender<Vec<u8>>,
    /// PTY/SSH -> UI（终端输出）
    pub output_rx: mpsc::Receiver<Vec<u8>>,
}
```

### 3.3 消息流序列图

**连接流程：**

```
UI Thread              Tokio Runtime           Remote Server
    │                       │                       │
    │─ AppCommand::Connect ─▶                       │
    │                       │── TCP connect ────────▶│
    │                       │◀─ TCP established ────│
    │                       │── SSH handshake ──────▶│
    │                       │◀─ SSH success ────────│
    │                       │── authenticate ───────▶│
    │                       │◀─ auth success ───────│
    │                       │── open PTY channel ───▶│
    │                       │◀─ channel opened ─────│
    │                       │                       │
    │◀─ AppEvent::Connected(SessionChannels) ──│    │
    │                       │                       │
    │  [UI 创建 Tab + Terminal 实例]                 │
    │  [启用 input_tx/output_rx]                     │
```

**数据流（稳态）：**

```
用户按键              UI Thread              Tokio SSH Task       Remote
   │                    │                       │                   │
   │──KeyEvent──▶       │                       │                   │
   │              Terminal.handle_key()          │                   │
   │              返回 bytes                     │                   │
   │                    │─ input_tx.send(bytes) ▶│                   │
   │                    │                       │── channel.data ──▶│
   │                    │                       │◀─ channel.data ──│
   │                    │◀─ output_tx.send(data)│                   │
   │              [每帧 update() 中]             │                   │
   │              output_rx.try_recv()           │                   │
   │              Terminal.process_output(data)   │                   │
   │              egui 渲染 Terminal.render()     │                   │
```

### 3.4 关键设计决策：无 SharedState

现有 feature-design spec 用了 `Arc<Mutex<HashMap<SessionId, TerminalBuffer>>>` 共享状态。**MVP 改为纯消息传递，去掉 SharedState**：

- Terminal 实例只存在于 UI 线程，不需要 Arc/Mutex
- 后端通过 output_tx 发送原始字节，UI 线程在 update() 中 try_recv 并调用 terminal.process_output()
- 消除所有锁竞争，简化代码，避免复杂的锁顺序规则

```rust
/// App 主结构 — 只在 UI 线程访问，无锁
pub struct App {
    // 活跃标签页
    tabs: Vec<Tab>,
    active_tab: usize,

    // 全局通道
    command_tx: mpsc::Sender<AppCommand>,
    event_rx: mpsc::Receiver<AppEvent>,

    // 配置
    config: ConfigManager,
    settings: AppSettings,
    theme: Theme,

    // UI 状态
    show_connect_dialog: bool,
    show_settings_dialog: bool,
    show_sftp_panel: bool,
    notifications: Vec<Notification>,
}

pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub session_id: SessionId,
    pub terminal: Terminal,
    pub input_tx: mpsc::Sender<Vec<u8>>,
    pub output_rx: mpsc::Receiver<Vec<u8>>,
    pub status: ConnectionStatus,
    pub sftp: Option<SftpState>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. 拉取全局事件
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
        }

        // 2. 拉取每个标签的终端输出
        for tab in &mut self.tabs {
            while let Ok(data) = tab.output_rx.try_recv() {
                tab.terminal.process_output(&data);
            }
        }

        // 3. 渲染 UI
        self.render_ui(ctx);

        // 4. 如果有活跃连接，请求持续重绘
        if self.has_active_connections() {
            ctx.request_repaint();
        }
    }
}
```

---

## 四、状态机定义

### 4.1 连接状态机

```
                    ┌──────────────┐
                    │ Disconnected │◀──────────────────────────┐
                    └──────┬───────┘                            │
                           │ user: Connect                     │
                           ▼                                    │
                    ┌──────────────┐                            │
               ┌───│  Connecting  │───┐                        │
               │   └──────────────┘   │                        │
               │                      │                        │
          timeout/error           success                      │
               │                      │                        │
               ▼                      ▼                        │
        ┌──────────────┐     ┌──────────────┐                  │
        │    Error     │     │  Connected   │──────┐           │
        └──────┬───────┘     └──────┬───────┘      │           │
               │                    │          network         │
          user: Retry          user: Disconnect  error         │
               │                    │              │           │
               │                    │              ▼           │
               │                    │      ┌──────────────┐    │
               │                    │      │ Reconnecting │    │
               │                    │      └──────┬───────┘    │
               │                    │         ┌───┴───┐        │
               │                    │      success  max_retries│
               │                    │         │       │        │
               │                    │         ▼       │        │
               │                    │    Connected    │        │
               └────────────────────┴─────────────────┴────────┘
                         (全部回到 Disconnected)
```

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected {
        connected_at: Instant,
        server_banner: Option<String>,
    },
    Reconnecting {
        attempt: u32,
        max_attempts: u32,
        next_retry_at: Instant,
        last_error: String,
    },
    Error {
        error: String,
        can_retry: bool,
    },
}

impl ConnectionState {
    pub fn transition(&self, event: ConnectionEvent) -> Option<ConnectionState> {
        match (self, event) {
            (Disconnected, ConnectionEvent::UserConnect) =>
                Some(Connecting),

            (Connecting, ConnectionEvent::Success { banner }) =>
                Some(Connected {
                    connected_at: Instant::now(),
                    server_banner: banner,
                }),

            (Connecting, ConnectionEvent::Timeout) =>
                Some(Error {
                    error: "连接超时".into(),
                    can_retry: true,
                }),

            (Connecting, ConnectionEvent::AuthFailed(msg)) =>
                Some(Error {
                    error: msg,
                    can_retry: true,  // 可重输密码
                }),

            (Connected { .. }, ConnectionEvent::NetworkError(e)) =>
                Some(Reconnecting {
                    attempt: 1,
                    max_attempts: 3,
                    next_retry_at: Instant::now() + Duration::from_secs(2),
                    last_error: e,
                }),

            (Connected { .. }, ConnectionEvent::UserDisconnect) =>
                Some(Disconnected),

            (Reconnecting { attempt, max_attempts, .. },
             ConnectionEvent::Success { banner }) =>
                Some(Connected {
                    connected_at: Instant::now(),
                    server_banner: banner,
                }),

            (Reconnecting { attempt, max_attempts, .. },
             ConnectionEvent::NetworkError(e)) if *attempt < *max_attempts =>
                Some(Reconnecting {
                    attempt: attempt + 1,
                    max_attempts: *max_attempts,
                    next_retry_at: Instant::now() + backoff_delay(*attempt),
                    last_error: e,
                }),

            (Reconnecting { .. }, ConnectionEvent::NetworkError(e)) =>
                Some(Error {
                    error: format!("重连失败: {}", e),
                    can_retry: true,
                }),

            (Error { can_retry: true, .. }, ConnectionEvent::UserRetry) =>
                Some(Connecting),

            _ => None,  // 无效转换，忽略
        }
    }
}

/// 重连退避：2s, 4s, 8s
fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_secs(2u64.pow(attempt))
}
```

### 4.2 终端模式状态机

```rust
bitflags! {
    pub struct TerminalMode: u32 {
        /// 光标键模式 (DECCKM) — 影响方向键发送 CSI/SS3
        const CURSOR_KEYS_APPLICATION = 0x0001;
        /// 换行模式 (LNM) — LF 是否隐含 CR
        const LINE_FEED_NEW_LINE = 0x0002;
        /// 插入/替换模式 (IRM)
        const INSERT_MODE = 0x0004;
        /// 自动换行 (DECAWM)
        const AUTO_WRAP = 0x0008;
        /// 反显模式 (DECSCNM)
        const REVERSE_VIDEO = 0x0010;
        /// 光标可见 (DECTCEM)
        const CURSOR_VISIBLE = 0x0020;
        /// 备用屏幕 (alternate screen buffer)
        const ALTERNATE_SCREEN = 0x0040;
        /// 鼠标追踪（多个级别，MVP 只标记开关）
        const MOUSE_TRACKING = 0x0080;
        /// 括号粘贴模式
        const BRACKETED_PASTE = 0x0100;
    }
}

impl Default for TerminalMode {
    fn default() -> Self {
        Self::AUTO_WRAP | Self::CURSOR_VISIBLE
    }
}
```

### 4.3 UI 视图状态机

```rust
/// 主区域当前显示什么
pub enum MainView {
    /// 欢迎页（无活跃标签时）
    Welcome,
    /// 终端标签
    Terminal { tab_id: TabId },
    /// SFTP 面板（覆盖在终端下方）
    TerminalWithSftp { tab_id: TabId },
}

/// 模态对话框栈
pub enum ModalDialog {
    None,
    NewConnection(NewConnectionState),
    Settings(SettingsState),
    HostKeyConfirm {
        host: String,
        fingerprint: String,
        response_tx: oneshot::Sender<bool>,
    },
    PasswordPrompt {
        host: String,
        username: String,
        response_tx: oneshot::Sender<String>,
    },
    Error {
        title: String,
        message: String,
    },
}

---

## 五、配置与存储子系统

### 5.1 存储目录结构与平台路径

```rust
/// 平台相关的配置目录
/// Windows: %APPDATA%\rsterm\        (C:\Users\xxx\AppData\Roaming\rsterm\)
/// macOS:   ~/Library/Application Support/rsterm/
/// Linux:   ~/.config/rsterm/
///
/// 使用 dirs::config_dir() 获取基础路径

pub struct ConfigManager {
    base_dir: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Result<Self, ConfigError> {
        let base = dirs::config_dir()
            .ok_or(ConfigError::NoPlatformConfigDir)?
            .join("rsterm");

        // 首次运行：创建目录结构
        if !base.exists() {
            Self::initialize_dirs(&base)?;
        }

        Ok(Self { base_dir: base })
    }

    fn initialize_dirs(base: &Path) -> Result<(), ConfigError> {
        fs::create_dir_all(base)?;
        fs::create_dir_all(base.join("themes"))?;

        // 写入默认配置
        let default_settings = AppSettings::default();
        let toml = toml::to_string_pretty(&default_settings)?;
        fs::write(base.join("config.toml"), toml)?;

        // 写入空会话文件
        fs::write(base.join("sessions.toml"), "[sessions]\nlist = []\n")?;

        // 写入默认主题
        fs::write(base.join("themes").join("dark.toml"),
                  include_str!("../assets/themes/dark.toml"))?;
        fs::write(base.join("themes").join("light.toml"),
                  include_str!("../assets/themes/light.toml"))?;

        Ok(())
    }
}
```

**文件布局（MVP）：**

```
~/.config/rsterm/           (或平台等效路径)
├── config.toml             # 应用全局设置
├── sessions.toml           # 所有会话配置
├── known_hosts             # SSH 已知主机指纹
└── themes/
    ├── dark.toml
    └── light.toml
```

### 5.2 数据模型（完整 Serde 定义）

```rust
/// ===== 应用设置 (config.toml) =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub app: AppConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub ssh: SshConfig,
    #[serde(default)]
    pub sftp: SftpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_theme")]
    pub theme: String,                    // "dark" | "light" | 自定义名
    #[serde(default = "default_font")]
    pub font_family: String,              // "JetBrains Mono"
    #[serde(default = "default_font_size")]
    pub font_size: f32,                   // 14.0
    #[serde(default = "default_true")]
    pub check_update_on_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    #[serde(default = "default_scroll_buffer")]
    pub scroll_buffer_lines: usize,       // 10000
    #[serde(default)]
    pub copy_on_select: bool,             // false
    #[serde(default = "default_true")]
    pub right_click_paste: bool,          // true
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,             // "block" | "underline" | "bar"
    #[serde(default = "default_true")]
    pub cursor_blink: bool,               // true
    #[serde(default = "default_bell")]
    pub bell_style: String,               // "visual" | "audible" | "none"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    #[serde(default = "default_timeout")]
    pub connection_timeout_secs: u64,     // 30
    #[serde(default = "default_keepalive")]
    pub keepalive_interval_secs: u64,     // 30
    #[serde(default = "default_keepalive_max")]
    pub keepalive_max_count: u32,         // 3
    #[serde(default = "default_reconnect_max")]
    pub reconnect_max_attempts: u32,      // 3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpConfig {
    #[serde(default)]
    pub show_hidden_files: bool,
    #[serde(default = "default_true")]
    pub confirm_overwrite: bool,
    #[serde(default = "default_sftp_buffer")]
    pub transfer_buffer_size: usize,      // 65536 (64KB)
}

/// ===== 会话配置 (sessions.toml) =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsFile {
    pub sessions: SessionList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionList {
    pub list: Vec<SessionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub id: String,                       // UUID v4
    pub name: String,                     // 显示名称
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,                        // 22
    pub username: String,
    pub auth: AuthConfig,
    #[serde(default)]
    pub startup_command: Option<String>,   // 连接后自动执行
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub created_at: Option<String>,       // ISO 8601
    #[serde(default)]
    pub last_connected: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthConfig {
    #[serde(rename = "password")]
    Password {
        /// MVP: 明文存储（后续版本迁移到 keyring）
        /// 字段可选——如果为空，连接时弹出密码输入框
        password: Option<String>,
    },
    #[serde(rename = "key")]
    Key {
        key_path: String,
        /// 私钥密码短语（可选，同上）
        passphrase: Option<String>,
    },
}
```

**sessions.toml 实例：**

```toml
[sessions]

[[sessions.list]]
id = "550e8400-e29b-41d4-a716-446655440000"
name = "Production Web"
host = "192.168.1.100"
port = 22
username = "deploy"
created_at = "2026-03-21T10:00:00Z"
last_connected = "2026-03-21T15:30:00Z"

[sessions.list.auth]
type = "key"
key_path = "~/.ssh/id_ed25519"

[[sessions.list]]
id = "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
name = "Dev Server"
host = "10.0.0.50"
port = 2222
username = "dev"

[sessions.list.auth]
type = "password"
password = "changeme"
```

### 5.3 ConfigManager 完整实现契约

```rust
impl ConfigManager {
    // --- 设置 ---

    pub fn load_settings(&self) -> Result<AppSettings, ConfigError> {
        let path = self.base_dir.join("config.toml");
        if !path.exists() {
            return Ok(AppSettings::default());
        }
        let content = fs::read_to_string(&path)?;
        Ok(toml::from_str(&content)?)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(settings)?;
        // 原子写入：先写临时文件再 rename
        let tmp = self.base_dir.join("config.toml.tmp");
        fs::write(&tmp, &content)?;
        fs::rename(&tmp, self.base_dir.join("config.toml"))?;
        Ok(())
    }

    // --- 会话 ---

    pub fn load_sessions(&self) -> Result<Vec<SessionConfig>, ConfigError> {
        let path = self.base_dir.join("sessions.toml");
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = fs::read_to_string(&path)?;
        let file: SessionsFile = toml::from_str(&content)?;
        Ok(file.sessions.list)
    }

    pub fn save_session(&self, session: &SessionConfig) -> Result<(), ConfigError> {
        let mut sessions = self.load_sessions()?;
        // 更新或插入
        if let Some(existing) = sessions.iter_mut().find(|s| s.id == session.id) {
            *existing = session.clone();
        } else {
            sessions.push(session.clone());
        }
        self.write_sessions(&sessions)
    }

    pub fn delete_session(&self, id: &str) -> Result<(), ConfigError> {
        let mut sessions = self.load_sessions()?;
        sessions.retain(|s| s.id != id);
        self.write_sessions(&sessions)
    }

    fn write_sessions(&self, sessions: &[SessionConfig]) -> Result<(), ConfigError> {
        let file = SessionsFile {
            sessions: SessionList { list: sessions.to_vec() },
        };
        let content = toml::to_string_pretty(&file)?;
        let tmp = self.base_dir.join("sessions.toml.tmp");
        fs::write(&tmp, &content)?;
        fs::rename(&tmp, self.base_dir.join("sessions.toml"))?;
        Ok(())
    }

    // --- SSH 已知主机 ---

    pub fn known_hosts_path(&self) -> PathBuf {
        self.base_dir.join("known_hosts")
    }

    pub fn check_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> HostKeyStatus {
        // 读取 known_hosts，逐行比对 "host:port fingerprint"
        // 返回: Trusted / Unknown / Changed
    }

    pub fn trust_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Result<(), ConfigError> {
        // 追加到 known_hosts 文件
    }

    // --- 主题 ---

    pub fn load_theme(&self, name: &str) -> Result<Theme, ConfigError> {
        let path = self.base_dir.join("themes").join(format!("{}.toml", name));
        if !path.exists() {
            return Err(ConfigError::ThemeNotFound(name.into()));
        }
        let content = fs::read_to_string(&path)?;
        Ok(toml::from_str(&content)?)
    }
}

pub enum HostKeyStatus {
    Trusted,                              // 已知且匹配
    Unknown,                              // 首次连接
    Changed { old_fingerprint: String },  // 指纹变更（安全警告）
}
```

### 5.4 密码存储策略（MVP）

```
MVP 策略：简单分级

1. 优先：不存密码
   - auth.password = None
   - 每次连接弹出密码输入框
   - 最安全，适合高安全需求用户

2. 可选：明文存 TOML
   - auth.password = Some("xxx")
   - 用户主动选择"记住密码"时存储
   - UI 上明确警告"密码以明文存储"

3. 后续版本（v0.5.0）：
   - 系统 keyring 集成
   - AES-GCM 加密文件回退
```

---

## 六、错误处理策略

### 6.1 错误类型层级

```rust
/// ===== 顶层应用错误 =====

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Ssh(#[from] SshError),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error(transparent)]
    Sftp(#[from] SftpError),
    #[error(transparent)]
    Config(#[from] ConfigError),
}

/// ===== SSH 错误 =====

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("连接超时: {host}:{port}")]
    ConnectionTimeout { host: String, port: u16 },

    #[error("连接被拒绝: {host}:{port}")]
    ConnectionRefused { host: String, port: u16 },

    #[error("DNS 解析失败: {host}")]
    DnsResolution { host: String },

    #[error("认证失败: {reason}")]
    AuthFailed { reason: String },

    #[error("私钥文件不存在: {path}")]
    KeyFileNotFound { path: String },

    #[error("私钥格式错误或密码短语不正确")]
    InvalidKey,

    #[error("主机密钥验证失败: 指纹已变更")]
    HostKeyChanged,

    #[error("网络中断")]
    NetworkLost,

    #[error("SSH 协议错误: {0}")]
    Protocol(String),

    #[error("通道已关闭")]
    ChannelClosed,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Russh(#[from] russh::Error),
}

/// ===== Terminal 错误 =====

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("PTY 创建失败: {0}")]
    PtyCreation(String),

    #[error("PTY 进程已退出 (exit code: {code:?})")]
    PtyExited { code: Option<i32> },

    #[error("PTY 写入失败: {0}")]
    PtyWrite(#[from] std::io::Error),
}

/// ===== SFTP 错误 =====

#[derive(Debug, thiserror::Error)]
pub enum SftpError {
    #[error("SFTP 会话未建立")]
    NotConnected,

    #[error("权限不足: {path}")]
    PermissionDenied { path: String },

    #[error("文件不存在: {path}")]
    FileNotFound { path: String },

    #[error("磁盘空间不足")]
    DiskFull,

    #[error("传输中断: {reason}")]
    TransferInterrupted { reason: String },

    #[error("SFTP 协议错误: {0}")]
    Protocol(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// ===== Config 错误 =====

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("无法确定系统配置目录")]
    NoPlatformConfigDir,

    #[error("配置文件格式错误: {path}: {reason}")]
    ParseError { path: String, reason: String },

    #[error("主题不存在: {0}")]
    ThemeNotFound(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Toml(#[from] toml::de::Error),

    #[error(transparent)]
    TomlSer(#[from] toml::ser::Error),
}
```

### 6.2 错误到用户界面的映射

```rust
/// 将内部错误转换为用户可见的通知
pub struct UserNotification {
    pub level: NotificationLevel,
    pub title: String,
    pub message: String,
    pub action: Option<NotificationAction>,
}

pub enum NotificationLevel {
    Info,
    Warning,
    Error,
}

pub enum NotificationAction {
    Retry(AppCommand),           // "重试"按钮
    OpenSettings,                // "打开设置"
    Dismiss,                     // 仅"确定"
}

impl From<&SshError> for UserNotification {
    fn from(e: &SshError) -> Self {
        match e {
            SshError::ConnectionTimeout { host, port } => UserNotification {
                level: NotificationLevel::Error,
                title: "连接超时".into(),
                message: format!("无法连接到 {}:{}，请检查网络和主机地址", host, port),
                action: Some(NotificationAction::Retry(/* ... */)),
            },
            SshError::AuthFailed { reason } => UserNotification {
                level: NotificationLevel::Error,
                title: "认证失败".into(),
                message: format!("{}。请检查用户名和密码/密钥配置", reason),
                action: Some(NotificationAction::Retry(/* ... */)),
            },
            SshError::HostKeyChanged => UserNotification {
                level: NotificationLevel::Warning,
                title: "安全警告".into(),
                message: "远程主机的密钥指纹已变更，这可能意味着中间人攻击。\
                          如果你确认服务器已重装，请在 known_hosts 中删除旧记录。".into(),
                action: Some(NotificationAction::Dismiss),
            },
            SshError::NetworkLost => UserNotification {
                level: NotificationLevel::Warning,
                title: "网络中断".into(),
                message: "与远程主机的连接已断开，正在尝试重新连接...".into(),
                action: None,  // 自动重连，无需用户操作
            },
            _ => UserNotification {
                level: NotificationLevel::Error,
                title: "连接错误".into(),
                message: e.to_string(),
                action: Some(NotificationAction::Dismiss),
            },
        }
    }
}
```

### 6.3 错误传播路径

```
Tokio 后端任务                    UI 线程
     │                              │
     │  SshError / SftpError        │
     │         │                    │
     │         ▼                    │
     │  映射为 AppEvent::Error {    │
     │    session_id,               │
     │    error: AppError,          │
     │  }                           │
     │         │                    │
     │    event_tx.send() ─────────▶│
     │                              │  App::handle_event()
     │                              │       │
     │                              │       ▼
     │                              │  更新 ConnectionState
     │                              │  生成 UserNotification
     │                              │  推入 notifications 队列
     │                              │       │
     │                              │       ▼
     │                              │  UI 渲染通知 Toast/Dialog
```

**规则：**
- Tokio 任务内部**不 panic**，所有错误通过 event channel 传递
- UI 线程只收 `AppEvent`，不直接处理底层错误类型
- 致命错误（配置目录不可写、Tokio runtime 创建失败）= 启动时 panic 并显示 message box
- 非致命错误 = Toast 通知 + 自动/手动重试

---

## 七、跨平台适配方案

### 7.1 平台差异矩阵

| 差异点 | Windows | macOS | Linux |
|--------|---------|-------|-------|
| 默认 Shell | `powershell.exe` / `cmd.exe` | `/bin/zsh` | `$SHELL` 或 `/bin/bash` |
| 配置目录 | `%APPDATA%\rsterm\` | `~/Library/Application Support/rsterm/` | `~/.config/rsterm/` |
| 路径分隔符 | `\` (但 SSH 远端永远用 `/`) | `/` | `/` |
| 系统字体目录 | `C:\Windows\Fonts\` | `/Library/Fonts/`, `~/Library/Fonts/` | `/usr/share/fonts/`, `~/.local/share/fonts/` |
| 系统密钥环 | Windows Credential Manager | macOS Keychain | Secret Service (GNOME) / KWallet |
| SSH 密钥默认路径 | `%USERPROFILE%\.ssh\` | `~/.ssh/` | `~/.ssh/` |
| 剪贴板 | Win32 API | NSPasteboard | X11 Selection / Wayland |
| IME 输入法 | TSF/IMM32 | macOS IME | IBus/Fcitx |

### 7.2 平台抽象层

```rust
/// 平台抽象 — 集中处理所有平台差异
pub mod platform {
    use std::path::PathBuf;

    /// 获取默认 shell
    pub fn default_shell() -> String {
        #[cfg(target_os = "windows")]
        {
            // 优先 PowerShell 7+，回退 Windows PowerShell
            if which::which("pwsh").is_ok() {
                "pwsh".into()
            } else {
                "powershell.exe".into()
            }
        }

        #[cfg(target_os = "macos")]
        {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
        }

        #[cfg(target_os = "linux")]
        {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
        }
    }

    /// 获取默认 SSH 密钥搜索路径
    pub fn ssh_key_search_paths() -> Vec<PathBuf> {
        let ssh_dir = dirs::home_dir()
            .map(|h| h.join(".ssh"))
            .unwrap_or_default();

        vec![
            ssh_dir.join("id_ed25519"),
            ssh_dir.join("id_rsa"),
            ssh_dir.join("id_ecdsa"),
        ]
    }

    /// 展开路径中的 ~ 和环境变量
    pub fn expand_path(path: &str) -> PathBuf {
        if path.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                return home.join(&path[2..]);  // 跳过 "~/"
            }
        }

        // Windows: 展开 %VAR%
        #[cfg(target_os = "windows")]
        {
            shellexpand::full(path)
                .map(|s| PathBuf::from(s.as_ref()))
                .unwrap_or_else(|_| PathBuf::from(path))
        }

        #[cfg(not(target_os = "windows"))]
        {
            PathBuf::from(path)
        }
    }

    /// 本地 PTY 的环境变量设置
    pub fn pty_environment() -> Vec<(String, String)> {
        let mut env = vec![
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
        ];

        #[cfg(not(target_os = "windows"))]
        {
            if let Ok(lang) = std::env::var("LANG") {
                env.push(("LANG".into(), lang));
            } else {
                env.push(("LANG".into(), "en_US.UTF-8".into()));
            }
        }

        env
    }

    /// 在默认浏览器中打开 URL
    pub fn open_url(url: &str) -> Result<(), std::io::Error> {
        #[cfg(target_os = "windows")]
        { std::process::Command::new("cmd").args(["/C", "start", url]).spawn()?; }

        #[cfg(target_os = "macos")]
        { std::process::Command::new("open").arg(url).spawn()?; }

        #[cfg(target_os = "linux")]
        { std::process::Command::new("xdg-open").arg(url).spawn()?; }

        Ok(())
    }
}
```

### 7.3 字体加载策略

```rust
pub struct FontLoader;

impl FontLoader {
    /// 字体加载优先级：
    /// 1. 用户配置的字体（settings.font_family）
    /// 2. 内嵌字体（编译进二进制）
    /// 3. 系统等宽字体回退
    pub fn load_fonts(
        cc: &eframe::CreationContext,
        preferred: &str,
    ) {
        let mut fonts = egui::FontDefinitions::default();

        // 内嵌默认等宽字体（保证所有平台可用）
        // 推荐内嵌 JetBrains Mono（OFL 许可兼容 MIT）
        fonts.font_data.insert(
            "JetBrainsMono".into(),
            Arc::new(egui::FontData::from_static(
                include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")
            )),
        );
        fonts.font_data.insert(
            "JetBrainsMono-Bold".into(),
            Arc::new(egui::FontData::from_static(
                include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf")
            )),
        );

        // CJK 回退字体（保证中日韩字符渲染）
        #[cfg(target_os = "windows")]
        let cjk_fallback = Self::load_system_font("Microsoft YaHei");
        #[cfg(target_os = "macos")]
        let cjk_fallback = Self::load_system_font("PingFang SC");
        #[cfg(target_os = "linux")]
        let cjk_fallback = Self::load_system_font("Noto Sans CJK SC");

        if let Some(cjk_data) = cjk_fallback {
            fonts.font_data.insert("CJK-Fallback".into(), Arc::new(cjk_data));
        }

        // 设置字体族优先级
        fonts.families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "JetBrainsMono".into());

        // CJK 作为 fallback
        if fonts.font_data.contains_key("CJK-Fallback") {
            fonts.families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("CJK-Fallback".into());
        }

        cc.egui_ctx.set_fonts(fonts);
    }

    fn load_system_font(name: &str) -> Option<egui::FontData> {
        // 使用 font-kit 或直接读系统字体文件
        // 失败返回 None，不阻塞启动
        None // MVP: 先返回 None，依赖 egui 默认 CJK 回退
    }
}
```

---

## 八、egui UI 实现细节

### 8.1 eframe::App 主结构

```rust
pub fn main() -> eframe::Result<()> {
    // 1. 初始化 tracing
    let _guard = init_logging();

    // 2. 创建 Tokio runtime（独立线程）
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");

    // 3. 创建通道
    let (command_tx, command_rx) = tokio::sync::mpsc::channel::<AppCommand>(256);
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<AppEvent>(256);

    // 4. 启动后端命令处理循环
    let event_tx_clone = event_tx.clone();
    runtime.spawn(async move {
        backend_loop(command_rx, event_tx_clone).await;
    });

    // 5. 启动 eframe
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RSTerm")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "RSTerm",
        options,
        Box::new(move |cc| {
            FontLoader::load_fonts(cc, "JetBrains Mono");
            Ok(Box::new(App::new(command_tx, event_rx, runtime)))
        }),
    )
}

pub struct App {
    // --- 通道 ---
    command_tx: mpsc::Sender<AppCommand>,
    event_rx: mpsc::Receiver<AppEvent>,
    _runtime: tokio::runtime::Runtime,  // 持有所有权防 drop

    // --- 标签页 ---
    tabs: Vec<Tab>,
    active_tab_idx: Option<usize>,
    tab_id_counter: u64,

    // --- 会话 ---
    sessions: Vec<SessionConfig>,       // 已保存的会话列表
    config: ConfigManager,
    settings: AppSettings,

    // --- UI 状态 ---
    theme: Theme,
    modal: ModalDialog,
    notifications: VecDeque<UserNotification>,
    session_panel_width: f32,           // 左侧面板宽度（可拖拽）

    // --- SFTP ---
    sftp_panel_visible: bool,
    sftp_split_ratio: f32,              // SFTP 面板占比
}

impl App {
    pub fn new(
        command_tx: mpsc::Sender<AppCommand>,
        event_rx: mpsc::Receiver<AppEvent>,
        runtime: tokio::runtime::Runtime,
    ) -> Self {
        let config = ConfigManager::new().expect("无法初始化配置目录");
        let settings = config.load_settings().unwrap_or_default();
        let sessions = config.load_sessions().unwrap_or_default();
        let theme = config.load_theme(&settings.app.theme)
            .unwrap_or_else(|_| Theme::default_dark());

        Self {
            command_tx,
            event_rx,
            _runtime: runtime,
            tabs: vec![],
            active_tab_idx: None,
            tab_id_counter: 0,
            sessions,
            config,
            settings,
            theme,
            modal: ModalDialog::None,
            notifications: VecDeque::new(),
            session_panel_width: 200.0,
            sftp_panel_visible: false,
            sftp_split_ratio: 0.5,
        }
    }
}
```

### 8.2 主渲染循环

```rust
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ===== Phase 1: 拉取事件 =====
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_app_event(event);
        }

        // 拉取每个标签的终端输出
        for tab in &mut self.tabs {
            let mut count = 0;
            while let Ok(data) = tab.output_rx.try_recv() {
                tab.terminal.process_output(&data);
                count += 1;
                // 单帧最多处理 64 批数据，防止阻塞渲染
                if count >= 64 { break; }
            }
        }

        // ===== Phase 2: 渲染 =====

        // 2a. 顶部工具栏
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            self.render_toolbar(ui);
        });

        // 2b. 底部状态栏
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            self.render_statusbar(ui);
        });

        // 2c. 左侧会话面板
        egui::SidePanel::left("sessions")
            .default_width(self.session_panel_width)
            .resizable(true)
            .show(ctx, |ui| {
                self.render_session_panel(ui);
            });

        // 2d. 中央主区域
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.tabs.is_empty() {
                self.render_welcome(ui);
            } else {
                self.render_tab_bar(ui);
                self.render_active_terminal(ui);
            }
        });

        // 2e. 模态对话框（最顶层）
        self.render_modal(ctx);

        // 2f. 通知 Toast
        self.render_notifications(ctx);

        // ===== Phase 3: 请求重绘 =====
        if !self.tabs.is_empty() {
            // 有活跃终端时持续请求重绘
            ctx.request_repaint();
        }
    }
}
```

### 8.3 终端自定义 Widget

```rust
/// 终端视图 Widget — 核心渲染组件
pub struct TerminalWidget<'a> {
    terminal: &'a mut Terminal,
    theme: &'a Theme,
    fonts: &'a TerminalFonts,
    focused: bool,
}

impl<'a> TerminalWidget<'a> {
    pub fn new(
        terminal: &'a mut Terminal,
        theme: &'a Theme,
        fonts: &'a TerminalFonts,
        focused: bool,
    ) -> Self {
        Self { terminal, theme, fonts, focused }
    }
}

impl<'a> egui::Widget for TerminalWidget<'a> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let cell_size = self.fonts.cell_size;
        let available = ui.available_size();

        // 计算终端可见尺寸
        let visible_cols = (available.x / cell_size.x).floor() as u16;
        let visible_rows = (available.y / cell_size.y).floor() as u16;

        // 检测 resize
        let current_size = self.terminal.size();
        if visible_cols != current_size.cols || visible_rows != current_size.rows {
            self.terminal.resize(visible_cols, visible_rows);
            // 返回 resize 事件给调用者处理
        }

        // 分配绘制区域
        let desired_size = egui::vec2(
            visible_cols as f32 * cell_size.x,
            visible_rows as f32 * cell_size.y,
        );
        let (rect, response) = ui.allocate_exact_size(
            desired_size,
            egui::Sense::click_and_drag(),
        );

        let painter = ui.painter_at(rect);

        // 绘制背景
        painter.rect_filled(rect, 0.0, self.theme.terminal_bg());

        // 绘制网格内容
        let grid = self.terminal.render_snapshot();
        for row_idx in 0..visible_rows as usize {
            if row_idx >= grid.row_count() { break; }
            let row = grid.row(row_idx);
            let y = rect.min.y + row_idx as f32 * cell_size.y;

            for (col_idx, cell) in row.cells().enumerate() {
                let x = rect.min.x + col_idx as f32 * cell_size.x;
                let cell_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y), cell_size,
                );

                // 背景
                let bg = self.resolve_color(cell.bg, &self.theme);
                if bg != self.theme.terminal_bg() {
                    painter.rect_filled(cell_rect, 0.0, bg);
                }

                // 选择高亮
                if self.terminal.is_selected(row_idx, col_idx) {
                    painter.rect_filled(
                        cell_rect, 0.0,
                        self.theme.selection_bg(),
                    );
                }

                // 字符
                let ch = cell.char;
                if ch != ' ' && ch != '\0' {
                    let fg = self.resolve_color(cell.fg, &self.theme);
                    let font = self.fonts.get_font(cell.flags);
                    painter.text(
                        egui::pos2(x, y),
                        egui::Align2::LEFT_TOP,
                        ch.to_string(),
                        font.clone(),
                        fg,
                    );
                }
            }
        }

        // 光标
        if self.focused {
            self.render_cursor(&painter, &rect, cell_size);
        }

        // 处理鼠标交互
        self.handle_mouse(&response, &rect, cell_size);

        // 处理键盘输入
        if self.focused {
            self.handle_keyboard(ui);
        }

        response
    }
}
```

### 8.4 输入法 (IME) 处理

```rust
impl TerminalWidget<'_> {
    fn handle_keyboard(&mut self, ui: &mut egui::Ui) {
        // egui 通过 InputState 提供 IME 事件
        let events = ui.input(|i| i.events.clone());

        for event in &events {
            match event {
                // 普通按键
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    // 先检查是否是应用级快捷键
                    if self.is_app_shortcut(key, modifiers) {
                        continue;  // 由 App 层处理
                    }

                    // 转换为 terminal KeyEvent 并处理
                    if let Some(key_event) = self.convert_key(key, modifiers) {
                        if let Some(bytes) = self.terminal.handle_key(key_event) {
                            // 发送到 PTY (通过外部 input_tx)
                            self.pending_input.extend(bytes);
                        }
                    }
                }

                // IME 组合输入完成
                egui::Event::Text(text) => {
                    // IME 输出的完成文本直接发送
                    self.pending_input.extend(text.as_bytes());
                }

                // IME 预编辑（组合中）
                egui::Event::Ime(egui::ImeEvent::Preedit(text, cursor)) => {
                    // 在光标位置显示预编辑文本（浮动窗口）
                    self.ime_preedit = if text.is_empty() {
                        None
                    } else {
                        Some(ImePreedit {
                            text: text.clone(),
                            cursor: *cursor,
                        })
                    };
                }

                // IME 最终提交
                egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    self.ime_preedit = None;
                    self.pending_input.extend(text.as_bytes());
                }

                _ => {}
            }
        }
    }
}
```

### 8.5 标签栏实现

```rust
impl App {
    fn render_tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

            let mut close_tab: Option<usize> = None;
            let mut switch_to: Option<usize> = None;

            for (idx, tab) in self.tabs.iter().enumerate() {
                let is_active = self.active_tab_idx == Some(idx);

                // 标签按钮
                let label = format!("{} {}",
                    tab.status_icon(),  // 圆点颜色表示状态
                    tab.title,
                );

                let btn = egui::Button::new(&label)
                    .fill(if is_active {
                        self.theme.tab_active_bg()
                    } else {
                        self.theme.tab_inactive_bg()
                    })
                    .rounding(egui::Rounding::same(4.0));

                let response = ui.add(btn);

                if response.clicked() {
                    switch_to = Some(idx);
                }

                // 中键关闭
                if response.middle_clicked() {
                    close_tab = Some(idx);
                }

                // 右键菜单
                response.context_menu(|ui| {
                    if ui.button("关闭").clicked() {
                        close_tab = Some(idx);
                        ui.close_menu();
                    }
                    if ui.button("关闭其他").clicked() {
                        // 保留当前，关闭其他
                        ui.close_menu();
                    }
                });
            }

            // "+" 新建标签按钮
            if ui.button("+").clicked() {
                self.modal = ModalDialog::NewConnection(NewConnectionState::default());
            }

            // 应用状态变更
            if let Some(idx) = switch_to {
                self.active_tab_idx = Some(idx);
            }
            if let Some(idx) = close_tab {
                self.close_tab(idx);
            }
        });
    }

    fn render_active_terminal(&mut self, ui: &mut egui::Ui) {
        if let Some(idx) = self.active_tab_idx {
            if let Some(tab) = self.tabs.get_mut(idx) {
                let fonts = TerminalFonts::new(
                    &self.settings.app.font_family,
                    self.settings.app.font_size,
                );

                let widget = TerminalWidget::new(
                    &mut tab.terminal,
                    &self.theme,
                    &fonts,
                    true,  // focused
                );

                let response = ui.add(widget);

                // 发送 pending input 到 PTY
                // (从 widget 收集待发送的输入字节)
            }
        }
    }
}
```

### 8.6 新建连接对话框

```rust
pub struct NewConnectionState {
    pub name: String,
    pub host: String,
    pub port: String,           // String 方便输入，验证时转 u16
    pub username: String,
    pub auth_type: AuthType,    // Password / Key
    pub password: String,
    pub key_path: String,
    pub save_session: bool,
    pub remember_password: bool,
    pub error_message: Option<String>,
}

pub enum AuthType { Password, Key }

impl App {
    fn render_connect_dialog(&mut self, ctx: &egui::Context) {
        let ModalDialog::NewConnection(ref mut state) = self.modal else { return };

        egui::Window::new("新建连接")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("connect_form")
                    .num_columns(2)
                    .spacing([8.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("名称:");
                        ui.text_edit_singleline(&mut state.name);
                        ui.end_row();

                        ui.label("主机:");
                        ui.text_edit_singleline(&mut state.host);
                        ui.end_row();

                        ui.label("端口:");
                        ui.add(egui::TextEdit::singleline(&mut state.port)
                            .desired_width(60.0));
                        ui.end_row();

                        ui.label("用户名:");
                        ui.text_edit_singleline(&mut state.username);
                        ui.end_row();

                        ui.label("认证方式:");
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut state.auth_type,
                                AuthType::Password, "密码"
                            );
                            ui.selectable_value(
                                &mut state.auth_type,
                                AuthType::Key, "密钥"
                            );
                        });
                        ui.end_row();

                        match state.auth_type {
                            AuthType::Password => {
                                ui.label("密码:");
                                ui.add(egui::TextEdit::singleline(&mut state.password)
                                    .password(true));
                                ui.end_row();
                            }
                            AuthType::Key => {
                                ui.label("私钥路径:");
                                ui.horizontal(|ui| {
                                    ui.text_edit_singleline(&mut state.key_path);
                                    if ui.button("浏览...").clicked() {
                                        // 打开文件选择对话框
                                        if let Some(path) = rfd::FileDialog::new()
                                            .set_directory(
                                                dirs::home_dir()
                                                    .unwrap_or_default()
                                                    .join(".ssh")
                                            )
                                            .pick_file()
                                        {
                                            state.key_path = path.display().to_string();
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                        }

                        ui.label("");
                        ui.checkbox(&mut state.save_session, "保存会话");
                        ui.end_row();
                    });

                // 错误提示
                if let Some(ref err) = state.error_message {
                    ui.colored_label(egui::Color32::RED, err);
                }

                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("连接").clicked() {
                        self.do_connect();
                    }
                    if ui.button("取消").clicked() {
                        self.modal = ModalDialog::None;
                    }
                });
            });
    }
}
```

---

## 九、性能目标与约束

### 9.1 量化指标

| 指标 | 目标值 | 测量方式 |
|------|--------|----------|
| 冷启动时间 | < 500ms（显示窗口） | 从进程启动到第一帧渲染 |
| 热连接时间 | < 2s（含 SSH 握手） | 从点击连接到终端可输入 |
| 内存基线 | < 40MB（空闲，无连接） | 任务管理器/htop 测量 RSS |
| 单连接内存增量 | < 15MB/连接 | 含终端缓冲区 10000 行 |
| 终端渲染帧率 | ≥ 30fps（大量输出时） | egui frame time |
| 输入延迟 | < 16ms（按键到屏幕） | 本地 PTY 回显测量 |
| 二进制体积 | < 15MB (release strip) | 编译后文件大小 |

### 9.2 终端渲染性能策略

```rust
/// 渲染优化核心：避免每帧绘制全部单元格

impl App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ...

        // 有活跃连接时，用 request_repaint_after 代替连续重绘
        // 避免空闲时 CPU 100%
        if self.has_active_connections() {
            // 无新数据时 30fps，有新数据时立即重绘
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

/// 大量输出时的批量处理策略
impl Tab {
    /// 单帧最多处理的输出数据量
    const MAX_OUTPUT_PER_FRAME: usize = 256 * 1024;  // 256KB

    fn drain_output(&mut self) {
        let mut total = 0;
        while let Ok(data) = self.output_rx.try_recv() {
            total += data.len();
            self.terminal.process_output(&data);

            if total >= Self::MAX_OUTPUT_PER_FRAME {
                break;  // 剩余数据下一帧处理，保持 UI 响应
            }
        }
    }
}
```

### 9.3 内存管理

```rust
/// 滚动缓冲区内存控制
impl ScrollBuffer {
    /// 默认 10000 行，每行约 200 字节 (80 cols * Cell)
    /// Cell = char(4B) + fg(4B) + bg(4B) + flags(1B) + width(1B) + padding(2B) = 16B
    /// 80 cols * 16B = 1280B/行
    /// 10000 行 ≈ 12.2MB — 在 15MB 预算内
    ///
    /// 超出 max_lines 时丢弃最旧的行

    pub fn push_line(&mut self, line: Row) {
        if self.lines.len() >= self.max_lines {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

/// Cell 内存优化：紧凑布局
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Cell {
    pub char: char,         // 4 bytes
    pub fg: PackedColor,    // 4 bytes (RGBA u32)
    pub bg: PackedColor,    // 4 bytes
    pub flags: CellFlags,   // 1 byte
    pub width: u8,          // 1 byte
    _padding: [u8; 2],      // 对齐到 16 bytes
}
// sizeof(Cell) = 16 bytes

/// 颜色压缩存储
#[derive(Clone, Copy, PartialEq)]
pub struct PackedColor(u32);

impl PackedColor {
    /// 使用索引颜色（0-255）节省空间
    /// 高 8 位 = 类型标记：0=默认, 1=索引色, 2=RGB
    /// 索引色: 0x01_00_00_XX
    /// RGB:    0x02_RR_GG_BB
    pub const DEFAULT_FG: Self = Self(0x00_000000);
    pub const DEFAULT_BG: Self = Self(0x00_000001);

    pub fn indexed(idx: u8) -> Self {
        Self(0x01_000000 | idx as u32)
    }

    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(0x02_000000 | (r as u32) << 16 | (g as u32) << 8 | b as u32)
    }

    pub fn to_egui_color(&self, theme: &Theme) -> egui::Color32 {
        match self.0 >> 24 {
            0 => {
                if self.0 == 0 { theme.terminal_fg() }
                else { theme.terminal_bg() }
            }
            1 => theme.indexed_color((self.0 & 0xFF) as u8),
            2 => egui::Color32::from_rgb(
                ((self.0 >> 16) & 0xFF) as u8,
                ((self.0 >> 8) & 0xFF) as u8,
                (self.0 & 0xFF) as u8,
            ),
            _ => theme.terminal_fg(),
        }
    }
}
```

### 9.4 启动优化

```
启动时序：

T+0ms    进程启动
T+5ms    初始化 tracing
T+10ms   创建 Tokio runtime（后台线程池）
T+15ms   加载 config.toml（小文件，< 1ms）
T+20ms   加载 sessions.toml
T+30ms   创建 eframe 窗口
T+80ms   加载内嵌字体（编译时 include_bytes!）
T+100ms  第一帧渲染 → 窗口可见
         （显示欢迎页或上次会话列表）
T+100ms+ 后台异步加载：CJK 系统字体、主题文件

关键：字体和配置在主线程同步加载，但都是本地文件 + 内嵌数据
CJK 系统字体查找较慢，放到后台，首帧可能 CJK 字符空白，
几百毫秒后补上。
```

---

## 十、测试策略

### 10.1 可测试性设计原则

```
核心原则：所有 I/O 边界都有 trait 抽象，可 mock

┌──────────────────────────────────────────────────┐
│                    可测试层                         │
│                                                    │
│  Terminal::process_output(&[u8])   ← 纯逻辑       │
│  Terminal::handle_key(KeyEvent)    ← 纯逻辑       │
│  Grid 内容断言                      ← 纯数据       │
│  ConnectionState::transition()     ← 纯状态机     │
│  ConfigManager (用 tempdir)        ← 文件系统隔离  │
│  TOML 序列化/反序列化              ← 纯数据       │
│                                                    │
├──────────────────────────────────────────────────┤
│                    需 Mock 层                       │
│                                                    │
│  PtyBackend trait                  ← mock impl    │
│  SSH 连接                          ← mock server  │
│  SFTP 操作                         ← mock server  │
│  系统密钥环                         ← 不测(MVP)    │
│  egui 渲染                          ← 不测(手工)   │
│                                                    │
└──────────────────────────────────────────────────┘
```

### 10.2 单元测试规划

```rust
/// ===== Terminal 模块测试（覆盖最密集） =====

#[cfg(test)]
mod terminal_tests {
    /// VT100 解析
    #[test]
    fn test_print_ascii() {
        let mut term = Terminal::new(80, 24);
        term.process_output(b"Hello");
        assert_eq!(term.cell_at(0, 0).char, 'H');
        assert_eq!(term.cell_at(0, 4).char, 'o');
    }

    #[test]
    fn test_cjk_double_width() {
        let mut term = Terminal::new(80, 24);
        term.process_output("你好".as_bytes());
        assert_eq!(term.cell_at(0, 0).char, '你');
        assert_eq!(term.cell_at(0, 0).width, 2);
        assert_eq!(term.cell_at(0, 2).char, '好');
    }

    #[test]
    fn test_sgr_colors() {
        let mut term = Terminal::new(80, 24);
        // ESC[31m = red foreground
        term.process_output(b"\x1b[31mRED\x1b[0m");
        assert_eq!(term.cell_at(0, 0).fg, PackedColor::indexed(1)); // red
    }

    #[test]
    fn test_cursor_movement() {
        let mut term = Terminal::new(80, 24);
        term.process_output(b"ABC\x1b[2D");  // 左移2
        assert_eq!(term.cursor_position(), Position { row: 0, col: 1 });
    }

    #[test]
    fn test_scroll_region() {
        let mut term = Terminal::new(80, 24);
        term.process_output(b"\x1b[5;20r");  // 滚动区域 5-20
        // 在第20行LF应触发区域内滚动
    }

    #[test]
    fn test_alternate_screen() {
        let mut term = Terminal::new(80, 24);
        term.process_output(b"main screen");
        term.process_output(b"\x1b[?1049h");  // 进入备用屏
        assert!(term.mode().contains(TerminalMode::ALTERNATE_SCREEN));
        term.process_output(b"alt screen");
        term.process_output(b"\x1b[?1049l");  // 退出备用屏
        assert_eq!(term.cell_at(0, 0).char, 'm');  // 恢复主屏
    }

    #[test]
    fn test_line_wrap() {
        let mut term = Terminal::new(5, 3);   // 极小终端
        term.process_output(b"ABCDEFGH");
        assert_eq!(term.cell_at(0, 4).char, 'E');  // 第一行末
        assert_eq!(term.cell_at(1, 0).char, 'F');  // 换行
    }

    #[test]
    fn test_scrollback_limit() {
        let mut term = Terminal::new(80, 3);
        term.set_scrollback_limit(5);
        // 输出超过 3+5=8 行
        for i in 0..20 {
            term.process_output(format!("line {}\n", i).as_bytes());
        }
        assert!(term.scrollback_len() <= 5);
    }

    /// 键盘输入
    #[test]
    fn test_key_arrow_up() {
        let mut term = Terminal::new(80, 24);
        let bytes = term.handle_key(KeyEvent::arrow_up());
        assert_eq!(bytes.unwrap(), b"\x1b[A");
    }

    #[test]
    fn test_key_ctrl_c() {
        let mut term = Terminal::new(80, 24);
        let bytes = term.handle_key(KeyEvent::ctrl('c'));
        assert_eq!(bytes.unwrap(), &[0x03]);
    }

    /// 选择
    #[test]
    fn test_word_selection() {
        let mut term = Terminal::new(80, 24);
        term.process_output(b"hello world");
        term.start_selection(Position { row: 0, col: 0 }, SelectionMode::Word);
        term.update_selection(Position { row: 0, col: 3 });
        assert_eq!(term.get_selection_text().unwrap(), "hello");
    }
}

/// ===== Config 模块测试 =====

#[cfg(test)]
mod config_tests {
    use tempfile::TempDir;

    fn test_config() -> (ConfigManager, TempDir) {
        let dir = TempDir::new().unwrap();
        let mgr = ConfigManager::with_base_dir(dir.path().to_path_buf()).unwrap();
        (mgr, dir)
    }

    #[test]
    fn test_default_settings() {
        let (mgr, _dir) = test_config();
        let settings = mgr.load_settings().unwrap();
        assert_eq!(settings.app.theme, "dark");
        assert_eq!(settings.app.font_size, 14.0);
    }

    #[test]
    fn test_save_load_session() {
        let (mgr, _dir) = test_config();
        let session = SessionConfig {
            id: "test-id".into(),
            name: "Test".into(),
            host: "localhost".into(),
            port: 22,
            username: "user".into(),
            auth: AuthConfig::Password { password: None },
            ..Default::default()
        };
        mgr.save_session(&session).unwrap();
        let loaded = mgr.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].host, "localhost");
    }

    #[test]
    fn test_delete_session() {
        let (mgr, _dir) = test_config();
        // save 2, delete 1, verify 1 remains
    }

    #[test]
    fn test_known_hosts() {
        let (mgr, _dir) = test_config();
        assert_eq!(
            mgr.check_host_key("example.com", 22, "SHA256:xxx"),
            HostKeyStatus::Unknown,
        );
        mgr.trust_host_key("example.com", 22, "SHA256:xxx").unwrap();
        assert_eq!(
            mgr.check_host_key("example.com", 22, "SHA256:xxx"),
            HostKeyStatus::Trusted,
        );
    }

    #[test]
    fn test_toml_roundtrip() {
        let settings = AppSettings::default();
        let toml_str = toml::to_string_pretty(&settings).unwrap();
        let parsed: AppSettings = toml::from_str(&toml_str).unwrap();
        assert_eq!(settings.app.font_size, parsed.app.font_size);
    }
}

/// ===== 状态机测试 =====

#[cfg(test)]
mod state_tests {
    #[test]
    fn test_connection_happy_path() {
        let state = ConnectionState::Disconnected;
        let state = state.transition(ConnectionEvent::UserConnect).unwrap();
        assert!(matches!(state, ConnectionState::Connecting));
        let state = state.transition(ConnectionEvent::Success { banner: None }).unwrap();
        assert!(matches!(state, ConnectionState::Connected { .. }));
    }

    #[test]
    fn test_reconnection_backoff() {
        let state = ConnectionState::Connected {
            connected_at: Instant::now(),
            server_banner: None,
        };
        let state = state.transition(ConnectionEvent::NetworkError("lost".into())).unwrap();
        assert!(matches!(state, ConnectionState::Reconnecting { attempt: 1, .. }));

        let state = state.transition(ConnectionEvent::NetworkError("lost".into())).unwrap();
        assert!(matches!(state, ConnectionState::Reconnecting { attempt: 2, .. }));
    }

    #[test]
    fn test_invalid_transitions_ignored() {
        let state = ConnectionState::Disconnected;
        assert!(state.transition(ConnectionEvent::Success { banner: None }).is_none());
    }
}
```

### 10.3 集成测试

```rust
/// ===== SSH 集成测试 =====
/// 使用 russh 内置的 test server 或 Docker openssh-server

#[cfg(test)]
mod ssh_integration {
    /// 需要环境变量 RSTERM_TEST_SSH_HOST 等
    /// CI 中启动 Docker: openssh-server

    #[tokio::test]
    #[ignore]  // 需要外部环境，默认跳过
    async fn test_password_auth() {
        let host = std::env::var("RSTERM_TEST_SSH_HOST")
            .unwrap_or("localhost".into());
        let service = SshService::new(/* ... */);
        let session = service.connect(&SessionConfig {
            host,
            port: 2222,
            username: "test".into(),
            auth: AuthConfig::Password {
                password: Some("test".into())
            },
            ..Default::default()
        }, Size { cols: 80, rows: 24, .. }).await.unwrap();

        assert!(session.is_alive());
        session.disconnect().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_terminal_echo() {
        // 连接 -> 发送 "echo hello\n" -> 验证输出包含 "hello"
    }
}
```

### 10.4 测试执行计划

```
日常开发：
  cargo test                    # 单元测试（秒级）

完整验证：
  cargo test                    # 单元测试
  cargo test -- --ignored       # 集成测试（需 Docker）
  cargo clippy                  # lint
  cargo fmt --check             # 格式检查

CI 流水线（后续添加）：
  1. cargo fmt --check
  2. cargo clippy -- -D warnings
  3. cargo test
  4. docker compose up -d openssh-server
  5. cargo test -- --ignored
  6. cargo build --release
```

---

## 十一、开发顺序与里程碑

MVP 开发分为 6 个里程碑，线性推进：

### M1: 骨架搭建

```
├─ Cargo.toml 添加所有 MVP 依赖
├─ 创建模块目录结构 (空 mod.rs)
├─ main.rs: eframe 启动 → 空白窗口
├─ tracing 初始化
├─ ConfigManager: 目录创建 + 默认配置
├─ Theme 加载 + 字体加载
└─ 验收：程序启动，显示窗口，加载配置
```

### M2: 终端核心

```
├─ Terminal struct + Grid + Cell + Cursor
├─ VTE 解析器集成 (Perform trait)
├─ 基础 CSI 处理：光标移动、擦除、SGR 颜色
├─ TerminalWidget: egui 单元格渲染
├─ 键盘输入 → 字节转换
├─ 滚动缓冲 (ScrollBuffer)
├─ 单元测试：VT100 序列全覆盖
└─ 验收：嵌入一个本地 PTY，可交互使用
```

### M3: 本地 PTY

```
├─ PtyBackend trait 定义
├─ LocalPty: portable-pty 集成
├─ Tokio 读写循环 (pty_read_loop / pty_write_loop)
├─ Tab 结构 + 标签栏 UI
├─ 新建本地终端标签
├─ 终端 resize 联动 PTY resize
└─ 验收：可打开多个本地终端标签，正常交互
```

### M4: SSH 连接

```
├─ SshService: russh 集成
├─ 密码认证 + 公钥认证
├─ SshSession: input_tx/output_rx 通道
├─ 已知主机管理 (known_hosts)
├─ 新建连接对话框 UI
├─ 连接状态机 (ConnectionState)
├─ 心跳保活 (keepalive loop)
├─ 断线检测 + 自动重连
└─ 验收：可 SSH 连接远程服务器，密码/密钥登录
```

### M5: 会话管理 + 配置持久化

```
├─ SessionConfig TOML 读写
├─ 左侧会话面板 (扁平列表)
├─ 双击连接 / 右键菜单
├─ 保存/编辑/删除会话
├─ 设置对话框 (字体、主题、终端参数)
├─ 深色/浅色主题切换
├─ 状态栏 (连接信息 + 延迟)
└─ 验收：会话可保存、加载、编辑，设置持久化
```

### M6: SFTP 基础 + 收尾

```
├─ SftpClient: russh-sftp 集成
├─ SFTP 面板 UI (双面板，文件列表)
├─ 列目录 + 目录导航
├─ 单文件上传/下载（进度条）
├─ mkdir / delete / rename
├─ 全功能冒烟测试
├─ 跨平台验证 (Windows + 一个 Linux)
└─ 验收：完整 MVP 功能可用
```

---

*文档完成。下一步：根据此规格书编写实现计划。*
```
