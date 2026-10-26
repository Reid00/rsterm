use std::fmt;

use super::InspectionError;

pub const MAX_CONNECTIONS: usize = 1000;

#[derive(Clone, Default)]
pub struct NetworkSnapshot {
    pub connections: Vec<ConnectionRow>,
    pub truncated: bool,
    pub notes: Vec<String>,
}

impl fmt::Debug for NetworkSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkSnapshot")
            .field("connections", &self.connections.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ConnectionRow {
    pub protocol: String,
    pub state: String,
    pub local_address: String,
    pub remote_address: String,
    pub process: Option<String>,
    pub pid: Option<u32>,
}

impl fmt::Debug for ConnectionRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConnectionRow([redacted])")
    }
}

pub fn parse_ss(bytes: &[u8]) -> Result<NetworkSnapshot, InspectionError> {
    let text = std::str::from_utf8(bytes).map_err(|_| InspectionError::InvalidData)?;
    let mut lines = text.lines();
    if lines.next() != Some("RSTERM_NETWORK_1") {
        return Err(InspectionError::InvalidData);
    }
    let mut snapshot = NetworkSnapshot::default();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 6 || !matches!(fields[0], "tcp" | "udp" | "tcp6" | "udp6") {
            return Err(InspectionError::InvalidData);
        }
        if snapshot.connections.len() == MAX_CONNECTIONS {
            snapshot.truncated = true;
            break;
        }
        let process = line
            .split_once("users:((\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(name, _)| name.to_owned());
        let pid = line
            .split_once("pid=")
            .and_then(|(_, rest)| rest.split(|ch: char| !ch.is_ascii_digit()).next())
            .and_then(|value| value.parse().ok());
        snapshot.connections.push(ConnectionRow {
            protocol: fields[0].to_ascii_uppercase(),
            state: fields[1].to_owned(),
            local_address: fields[4].to_owned(),
            remote_address: fields[5].to_owned(),
            process,
            pid,
        });
    }
    snapshot
        .notes
        .push("Processes are shown only when visible to the connection account".into());
    Ok(snapshot)
}

pub fn parse_lsof(bytes: &[u8]) -> Result<NetworkSnapshot, InspectionError> {
    let text = std::str::from_utf8(bytes).map_err(|_| InspectionError::InvalidData)?;
    let mut snapshot = NetworkSnapshot::default();
    let mut pid = None;
    let mut process = None;
    let mut row = ConnectionRow::default();
    let flush = |row: &mut ConnectionRow, snapshot: &mut NetworkSnapshot| {
        if !row.local_address.is_empty() && matches!(row.protocol.as_str(), "TCP" | "UDP") {
            if snapshot.connections.len() < MAX_CONNECTIONS {
                snapshot.connections.push(row.clone());
            } else {
                snapshot.truncated = true;
            }
        }
        *row = ConnectionRow::default();
    };
    for line in text.lines().filter(|line| !line.is_empty()) {
        if !line.as_bytes()[0].is_ascii() {
            return Err(InspectionError::InvalidData);
        }
        let (field, value) = line.split_at(1);
        match field {
            "p" => {
                flush(&mut row, &mut snapshot);
                pid = Some(value.parse().map_err(|_| InspectionError::InvalidData)?);
                process = None;
            }
            "c" => process = Some(value.to_owned()),
            "f" => {
                flush(&mut row, &mut snapshot);
                row.pid = pid;
                row.process = process.clone();
            }
            "P" => row.protocol = value.to_owned(),
            "n" => {
                let (local, remote) = value.split_once("->").unwrap_or((value, "*:*"));
                row.local_address = local.to_owned();
                row.remote_address = remote.to_owned();
                row.pid = pid;
                row.process = process.clone();
            }
            "T" => {
                if let Some(state) = value.strip_prefix("ST=") {
                    row.state = state.to_owned();
                }
            }
            _ => {}
        }
    }
    flush(&mut row, &mut snapshot);
    if snapshot.connections.iter().any(|row| row.state.is_empty()) {
        for row in &mut snapshot.connections {
            if row.state.is_empty() {
                row.state = "—".into();
            }
        }
    }
    snapshot
        .notes
        .push("Only TCP/UDP sockets and visible process names are included".into());
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_ss_ipv6_and_process_visibility() {
        let snapshot = parse_ss(b"RSTERM_NETWORK_1\ntcp ESTAB 0 0 [::1]:22 [::1]:1234 users:((\"sshd\",pid=42,fd=3))\nudp UNCONN 0 0 0.0.0.0:53 0.0.0.0:*\n").unwrap();
        assert_eq!(snapshot.connections[0].pid, Some(42));
        assert_eq!(snapshot.connections[0].process.as_deref(), Some("sshd"));
        assert_eq!(snapshot.connections[0].remote_address, "[::1]:1234");
        assert_eq!(snapshot.connections[1].pid, None);
    }

    #[test]
    fn lsof_machine_fields_keep_sockets_separate() {
        let snapshot = parse_lsof(b"p42\ncserver\nf3\nPTCP\nn[::1]:22->[::1]:1234\nTST=ESTABLISHED\nf4\nPUDP\nn*:5353\np88\ncworker\nf5\nPTCP\nn127.0.0.1:80\nTST=LISTEN\n").unwrap();
        assert_eq!(snapshot.connections.len(), 3);
        assert_eq!(snapshot.connections[1].protocol, "UDP");
        assert_eq!(snapshot.connections[1].process.as_deref(), Some("server"));
        assert_eq!(snapshot.connections[2].state, "LISTEN");
        assert!(!format!("{snapshot:?}").contains("server"));
    }

    #[test]
    fn malformed_machine_field_does_not_split_unicode() {
        assert_eq!(
            parse_lsof("énot-a-field\n".as_bytes()).unwrap_err(),
            InspectionError::InvalidData
        );
        assert_eq!(
            parse_ss(b"login banner\n").unwrap_err(),
            InspectionError::InvalidData
        );
    }

    #[test]
    fn connection_limit_is_explicit() {
        let text = format!(
            "RSTERM_NETWORK_1\n{}",
            "tcp LISTEN 0 0 *:22 *:*\n".repeat(MAX_CONNECTIONS + 1)
        );
        let snapshot = parse_ss(text.as_bytes()).unwrap();
        assert_eq!(snapshot.connections.len(), MAX_CONNECTIONS);
        assert!(snapshot.truncated);
    }
}
