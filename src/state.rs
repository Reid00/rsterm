#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Reconnecting { attempt: u32 },
    Error(String),
}

impl ConnectionState {
    pub fn label(&self) -> String {
        match self {
            Self::Connecting => "连接中".into(),
            Self::Connected => "已连接".into(),
            Self::Disconnected => "已断开".into(),
            Self::Reconnecting { attempt } => format!("重连中 · 第 {attempt} 次"),
            Self::Error(error) => error.clone(),
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }
}
