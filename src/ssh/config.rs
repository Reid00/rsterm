use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::config::{AuthConfig, SessionConfig};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshConfigId {
    pub source: PathBuf,
    pub alias: String,
}

#[derive(Debug, Clone)]
pub struct SshConfigSource {
    pub path: PathBuf,
    pub home: PathBuf,
}

impl SshConfigSource {
    pub fn load(&self) -> Result<SshConfigSnapshot, String> {
        match fs::read_to_string(&self.path) {
            Ok(text) => parse(&text, self),
            Err(error) => Err(format!("无法读取 SSH Config：{error}")),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SshConfigSnapshot {
    pub entries: Vec<SshConfigEntry>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SshConfigEntry {
    pub id: SshConfigId,
    pub profile: SessionConfig,
    pub identity_files: Vec<String>,
    pub warnings: Vec<String>,
    pub blocked: Vec<String>,
}

struct Directive {
    keyword: String,
    values: Vec<String>,
    line: usize,
}

struct Block {
    patterns: Option<Vec<String>>,
    directives: Vec<Directive>,
}

fn parse(text: &str, source: &SshConfigSource) -> Result<SshConfigSnapshot, String> {
    let mut blocks = vec![Block {
        patterns: None,
        directives: Vec::new(),
    }];
    let mut aliases = Vec::new();
    let mut seen_aliases = HashSet::new();
    let mut diagnostics = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let Some(directive) = tokenize(line, line_number)? else {
            continue;
        };
        match directive.keyword.as_str() {
            "host" => {
                if directive.values.iter().any(|pattern| pattern == "!") {
                    return Err(format!("SSH Config 第 {line_number} 行的 Host 模式无效"));
                }
                for alias in &directive.values {
                    if !alias.starts_with('!')
                        && !alias.contains(['*', '?'])
                        && seen_aliases.insert(alias.clone())
                    {
                        aliases.push(alias.clone());
                    }
                }
                blocks.push(Block {
                    patterns: Some(directive.values),
                    directives: Vec::new(),
                });
            }
            "include" | "match" => diagnostics.push(format!(
                "第 {line_number} 行包含未支持的 {}；该来源的所有主机暂不可连接或复制",
                directive.keyword
            )),
            _ => blocks.last_mut().unwrap().directives.push(directive),
        }
    }
    let entries = aliases
        .into_iter()
        .map(|alias| resolve(alias, source, &blocks, &diagnostics))
        .collect();
    Ok(SshConfigSnapshot {
        entries,
        diagnostics,
    })
}

fn tokenize(line: &str, line_number: usize) -> Result<Option<Directive>, String> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let key_end = line
        .find(|character: char| character.is_whitespace() || character == '=')
        .unwrap_or(line.len());
    let keyword = &line[..key_end];
    if keyword.is_empty() || !keyword.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(format!("SSH Config 第 {line_number} 行的选项名称无效"));
    }
    let mut remainder = line[key_end..].trim_start();
    if let Some(value) = remainder.strip_prefix('=') {
        remainder = value.trim_start();
    }
    let mut values = Vec::new();
    let mut value = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut characters = remainder.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let escaped = characters
                    .next()
                    .ok_or_else(|| format!("SSH Config 第 {line_number} 行有未完成的转义"))?;
                if !escaped.is_whitespace() && !matches!(escaped, '"' | '\\') {
                    value.push('\\');
                }
                value.push(escaped);
                started = true;
            }
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '#' if !quoted && !started => break,
            character if character.is_whitespace() && !quoted => {
                if started {
                    values.push(std::mem::take(&mut value));
                    started = false;
                }
            }
            character => {
                value.push(character);
                started = true;
            }
        }
    }
    if quoted {
        return Err(format!("SSH Config 第 {line_number} 行的引号未闭合"));
    }
    if started {
        values.push(value);
    }
    if values.is_empty() {
        return Err(format!("SSH Config 第 {line_number} 行缺少选项值"));
    }
    Ok(Some(Directive {
        keyword: keyword.to_ascii_lowercase(),
        values,
        line: line_number,
    }))
}

fn resolve(
    alias: String,
    source: &SshConfigSource,
    blocks: &[Block],
    diagnostics: &[String],
) -> SshConfigEntry {
    let mut profile = SessionConfig {
        name: alias.clone(),
        host: alias.clone(),
        ..Default::default()
    };
    let mut first = HashSet::new();
    let mut identity_files = Vec::new();
    let mut warnings = Vec::new();
    let mut blocked = diagnostics.to_vec();
    let mut identities_only = false;
    if !valid_atom(&alias) {
        blocked.push("Host 别名包含未支持的字符或展开表达式".into());
    }
    for block in blocks {
        if block
            .patterns
            .as_ref()
            .is_some_and(|patterns| !matches_host(patterns, &alias))
        {
            continue;
        }
        for directive in &block.directives {
            let keyword = directive.keyword.as_str();
            if keyword != "identityfile" && !first.insert(keyword) {
                continue;
            }
            match keyword {
                "hostname" | "user" | "port" | "identitiesonly" | "identityfile" => {
                    let [value] = directive.values.as_slice() else {
                        blocked.push(format!(
                            "第 {} 行的 {keyword} 必须只有一个值",
                            directive.line
                        ));
                        continue;
                    };
                    match keyword {
                        "hostname" | "user" => {
                            if valid_atom(value) {
                                if keyword == "hostname" {
                                    profile.host.clone_from(value);
                                } else {
                                    profile.username.clone_from(value);
                                }
                            } else {
                                blocked.push(format!(
                                    "第 {} 行的 {keyword} 包含无效值或未支持的展开表达式",
                                    directive.line
                                ));
                            }
                        }
                        "port" => match value.parse::<u16>() {
                            Ok(port) if port != 0 => profile.port = port,
                            _ => blocked.push(format!(
                                "第 {} 行的 Port 必须是 1–65535 之间的整数",
                                directive.line
                            )),
                        },
                        "identitiesonly" => match value.to_ascii_lowercase().as_str() {
                            "yes" => identities_only = true,
                            "no" => {}
                            _ => blocked.push(format!(
                                "第 {} 行的 IdentitiesOnly 必须为 yes 或 no",
                                directive.line
                            )),
                        },
                        "identityfile" => {
                            if value.eq_ignore_ascii_case("none") {
                                continue;
                            }
                            match identity_path(value, &source.home) {
                                Ok(path) if !identity_files.contains(&path) => {
                                    identity_files.push(path);
                                }
                                Ok(_) => {}
                                Err(reason) => blocked.push(format!(
                                    "第 {} 行的 IdentityFile {reason}",
                                    directive.line
                                )),
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                "proxyjump" | "proxycommand"
                    if directive.values.len() == 1
                        && directive.values[0].eq_ignore_ascii_case("none") => {}
                "setenv" | "addkeystoagent" | "usekeychain" => warnings.push(format!(
                    "未应用 {}；RSTerm 不执行该 OpenSSH 选项",
                    directive.keyword
                )),
                _ => blocked.push(format!(
                    "第 {} 行的 {} 尚未支持，请先在 SSH 配置中明确处理；不能直接连接或复制",
                    directive.line, directive.keyword
                )),
            }
        }
    }
    profile.auth = match identity_files.as_slice() {
        [key_path] => AuthConfig::Key {
            key_path: key_path.clone(),
            passphrase: None,
        },
        [] if !identities_only => AuthConfig::Password { password: None },
        _ => AuthConfig::Key {
            key_path: String::new(),
            passphrase: None,
        },
    };
    if profile.username.is_empty() {
        warnings.push("未指定 User，请在连接表单中填写用户名".into());
    }
    if identity_files.len() > 1 {
        warnings.push("配置包含多个 IdentityFile，请在连接表单中显式选择私钥".into());
    } else if identity_files.is_empty() {
        warnings.push("未指定可用私钥，请在连接表单中选择认证方式".into());
    }
    SshConfigEntry {
        id: SshConfigId {
            source: source.path.clone(),
            alias,
        },
        profile,
        identity_files,
        warnings,
        blocked,
    }
}

fn valid_atom(value: &str) -> bool {
    !value.is_empty()
        && !value.chars().any(|character| {
            character.is_whitespace()
                || character.is_control()
                || matches!(character, '%' | '$' | '\\' | '/' | '=' | '"' | '\'')
        })
}

fn identity_path(value: &str, home: &Path) -> Result<String, &'static str> {
    if value.is_empty() || value.contains(['%', '$']) || value.chars().any(char::is_control) {
        return Err("包含无效值或未支持的展开表达式");
    }
    let path = if let Some(relative) = value.strip_prefix("~/") {
        home.join(relative.trim_start_matches('/'))
    } else {
        PathBuf::from(value)
    };
    if !path.is_absolute() {
        return Err("必须是绝对路径或以 ~/ 开头的路径");
    }
    path.to_str()
        .filter(|path| !path.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or("路径无法表示为有效文本")
}

fn matches_host(patterns: &[String], alias: &str) -> bool {
    let mut positive = false;
    for pattern in patterns {
        if let Some(negative) = pattern.strip_prefix('!') {
            if wildcard_matches(negative, alias) {
                return false;
            }
        } else if wildcard_matches(pattern, alias) {
            positive = true;
        }
    }
    positive
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<_> = pattern.chars().collect();
    let value: Vec<_> = value.chars().collect();
    let (mut pattern_index, mut value_index) = (0, 0);
    let mut star = None;
    let mut retry_value = 0;
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star = Some(pattern_index);
            pattern_index += 1;
            retry_value = value_index;
        } else if let Some(star_index) = star {
            retry_value += 1;
            value_index = retry_value;
            pattern_index = star_index + 1;
        } else {
            return false;
        }
    }
    while pattern.get(pattern_index) == Some(&'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::{parse, SshConfigSnapshot, SshConfigSource};
    use crate::config::AuthConfig;
    use std::{fs, path::PathBuf};

    fn source() -> SshConfigSource {
        SshConfigSource {
            path: PathBuf::from("/fixture/home/.ssh/config"),
            home: PathBuf::from("/fixture/home"),
        }
    }

    fn parsed(text: &str) -> SshConfigSnapshot {
        parse(text, &source()).expect("parse synthetic config")
    }

    #[test]
    fn fourteen_static_aliases_load_without_reading_identity_files_or_changing_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = SshConfigSource {
            path: directory.path().join("config"),
            home: directory.path().to_owned(),
        };
        let mut text = String::new();
        for index in 0..14 {
            text.push_str(&format!(
                "Host host-{index}\n HostName host-{index}.test.invalid\n User fixture\n IdentityFile ~/.ssh/keys/nonexistent-{index}\n IdentitiesOnly yes\n"
            ));
        }
        fs::write(&source.path, &text).unwrap();
        let before_permissions = fs::metadata(&source.path).unwrap().permissions();
        let snapshot = source.load().unwrap();
        assert_eq!(snapshot.entries.len(), 14);
        assert!(snapshot.diagnostics.is_empty());
        for (index, entry) in snapshot.entries.iter().enumerate() {
            assert!(entry.blocked.is_empty());
            assert_eq!(entry.id.source, source.path);
            assert_eq!(entry.id.alias, format!("host-{index}"));
            assert_eq!(entry.profile.name, entry.id.alias);
            assert_eq!(entry.profile.host, format!("host-{index}.test.invalid"));
            assert_eq!(entry.profile.username, "fixture");
            assert_eq!(entry.profile.port, 22);
            assert!(entry.profile.id.is_empty());
            assert!(!entry.profile.remember_credentials);
            assert!(matches!(
                entry.profile.auth,
                AuthConfig::Key {
                    passphrase: None,
                    ..
                }
            ));
            assert!(!PathBuf::from(&entry.identity_files[0]).exists());
        }
        assert_eq!(fs::read_to_string(&source.path).unwrap(), text);
        assert_eq!(
            fs::metadata(&source.path).unwrap().permissions(),
            before_permissions
        );
    }

    #[test]
    fn quotes_comments_equals_and_escaped_spaces_preserve_values() {
        let snapshot = parsed(
            "# heading\nHoSt=alias other\nHOSTNAME = \"target.test.invalid\" # comment\nuSeR=fixture\nIdentityFile=\"/keys/a=b # key\"\nIdentityFile /keys/escaped\\ path\n",
        );
        assert_eq!(snapshot.entries.len(), 2);
        for entry in &snapshot.entries {
            assert!(entry.blocked.is_empty());
            assert_eq!(entry.profile.host, "target.test.invalid");
            assert_eq!(entry.profile.username, "fixture");
            assert_eq!(
                entry.identity_files,
                ["/keys/a=b # key", "/keys/escaped path"]
            );
        }
    }

    #[test]
    fn matching_blocks_preserve_first_scalars_and_add_identity_files() {
        let snapshot = parsed(
            "User global\nHost specific\n User ignored\n Port 2222\n IdentityFile /keys/specific\nHost * !excluded\n HostName shared.test.invalid\n Port 3333\n IdentityFile /keys/shared\nHost excluded\n User ignored-too\nHost s?ecific\n IdentityFile /keys/last\nHost *\n Port 4444\n",
        );
        assert_eq!(snapshot.entries.len(), 2);
        let specific = &snapshot.entries[0];
        assert_eq!(specific.profile.username, "global");
        assert_eq!(specific.profile.port, 2222);
        assert_eq!(specific.profile.host, "shared.test.invalid");
        assert_eq!(
            specific.identity_files,
            ["/keys/specific", "/keys/shared", "/keys/last"]
        );
        let excluded = &snapshot.entries[1];
        assert_eq!(excluded.profile.host, "excluded");
        assert_eq!(excluded.profile.port, 4444);
        assert!(excluded.identity_files.is_empty());
    }

    #[test]
    fn only_positive_literal_aliases_are_listed_and_duplicates_are_stable() {
        let snapshot = parsed(
            "Host * !excluded\n User fixture\nHost One two\n IdentityFile /keys/shared\nHost One\n Port 2200\n IdentityFile /keys/shared\nHost !two\n ProxyCommand bad\n",
        );
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(snapshot.entries[0].id.alias, "One");
        assert_eq!(snapshot.entries[1].id.alias, "two");
        assert_eq!(snapshot.entries[0].profile.port, 2200);
        assert_eq!(snapshot.entries[0].identity_files, ["/keys/shared"]);
        assert!(snapshot
            .entries
            .iter()
            .all(|entry| entry.blocked.is_empty()));
    }

    #[test]
    fn case_distinct_aliases_and_host_patterns_resolve_to_distinct_targets() {
        let snapshot = parsed(
            "HoSt One\n HostName upper.test.invalid\nHost one\n HostName lower.test.invalid\nHost O*\n User upper-user\nHost o*\n User lower-user\nHost * !One\n Port 2222\n",
        );
        assert_eq!(snapshot.entries.len(), 2);
        let upper = &snapshot.entries[0];
        let lower = &snapshot.entries[1];
        assert_eq!(upper.id.alias, "One");
        assert_eq!(lower.id.alias, "one");
        assert_ne!(upper.id, lower.id);
        assert_eq!(upper.profile.host, "upper.test.invalid");
        assert_eq!(lower.profile.host, "lower.test.invalid");
        assert_eq!(upper.profile.username, "upper-user");
        assert_eq!(lower.profile.username, "lower-user");
        assert_eq!(upper.profile.port, 22);
        assert_eq!(lower.profile.port, 2222);
        assert!(upper.blocked.is_empty());
        assert!(lower.blocked.is_empty());
    }

    #[test]
    fn quoted_and_unquoted_paths_preserve_backslashes_before_ordinary_characters() {
        for identity in [r"/keys/name\z", r#""/keys/name\z""#] {
            let snapshot = parsed(&format!("Host target\n IdentityFile {identity}\n"));
            let entry = &snapshot.entries[0];
            assert!(entry.blocked.is_empty());
            assert_eq!(entry.identity_files, [r"/keys/name\z"]);
            assert!(matches!(
                &entry.profile.auth,
                AuthConfig::Key { key_path, .. } if key_path == r"/keys/name\z"
            ));
        }
    }

    #[test]
    fn escaped_quotes_and_whitespace_remain_supported() {
        let snapshot = parsed(
            r#"Host target
 IdentityFile /keys/escaped\ path
 IdentityFile "/keys/escaped\"quote"
"#,
        );
        let entry = &snapshot.entries[0];
        assert!(entry.blocked.is_empty());
        assert_eq!(
            entry.identity_files,
            ["/keys/escaped path", "/keys/escaped\"quote"]
        );
    }

    #[test]
    fn quoted_and_unquoted_hostname_backslashes_block_instead_of_changing_target() {
        for hostname in [r"tar\get.test.invalid", r#""tar\get.test.invalid""#] {
            let snapshot = parsed(&format!("Host target\n HostName {hostname}\n"));
            let entry = &snapshot.entries[0];
            assert!(!entry.blocked.is_empty());
            assert_eq!(entry.profile.host, "target");
            assert_ne!(entry.profile.host, "target.test.invalid");
        }
    }

    #[test]
    fn late_include_and_match_block_every_alias_without_executing_commands() {
        for unsupported in ["Include config.d/*", "Match exec \"touch /never-run\""] {
            let snapshot = parsed(&format!(
                "Host before\n User fixture\n IdentityFile /keys/first\n{unsupported}\nHost after\n User fixture\n"
            ));
            assert_eq!(snapshot.entries.len(), 2);
            assert_eq!(snapshot.diagnostics.len(), 1);
            for entry in &snapshot.entries {
                assert_eq!(entry.blocked, snapshot.diagnostics);
            }
        }
    }

    #[test]
    fn unsupported_options_only_block_matching_entries_and_never_echo_values() {
        let snapshot = parsed(
            "Host proxied\n ProxyCommand command-with-secret-argument\nHost safe\n ProxyCommand none\n ProxyJump none\n SetEnv TOKEN=secret-value\n AddKeysToAgent yes\n UseKeychain yes\nHost host-key-rule\n HostKeyAlias another-host\nHost unknown\n CustomOption secret-option-value\n",
        );
        assert_eq!(snapshot.entries[0].blocked.len(), 1);
        assert!(!snapshot.entries[0].blocked[0].contains("secret-argument"));
        assert!(snapshot.entries[1].blocked.is_empty());
        assert_eq!(snapshot.entries[1].warnings.len(), 5);
        assert!(!snapshot.entries[1]
            .warnings
            .join(" ")
            .contains("secret-value"));
        assert!(snapshot.entries[1].profile.environment.is_empty());
        assert!(snapshot.entries[2].blocked[0].contains("hostkeyalias"));
        assert!(snapshot.entries[3].blocked[0].contains("customoption"));
        assert!(!snapshot.entries[3].blocked[0].contains("secret-option-value"));
    }

    #[test]
    fn ignored_later_scalar_options_do_not_replace_first_values() {
        let snapshot = parsed(
            "Host target\n HostName first.test.invalid\n HostName later.test.invalid\n User first-user\n User later-user\n Port 2222\n Port 3333\n ProxyJump none\n ProxyJump ignored-proxy\n",
        );
        let entry = &snapshot.entries[0];
        assert!(entry.blocked.is_empty());
        assert_eq!(entry.profile.host, "first.test.invalid");
        assert_eq!(entry.profile.username, "first-user");
        assert_eq!(entry.profile.port, 2222);
    }

    #[test]
    fn invalid_ports_and_scalar_arity_block_instead_of_connecting_to_defaults() {
        for option in [
            "Port 0",
            "Port 65536",
            "Port ssh",
            "Port 22 2200",
            "HostName one two",
            "User one two",
            "IdentitiesOnly maybe",
            "IdentityFile /one /two",
        ] {
            let snapshot = parsed(&format!("Host target\n {option}\n"));
            assert!(!snapshot.entries[0].blocked.is_empty(), "{option}");
        }
    }

    #[test]
    fn unresolved_target_identity_expansions_and_relative_paths_are_blocked() {
        for option in [
            "HostName %h.example",
            "HostName ${TARGET}",
            "User %r",
            "User ${REMOTE_USER}",
            "IdentityFile %d/.ssh/key",
            "IdentityFile ${HOME}/.ssh/key",
            "IdentityFile relative-key",
            "IdentityFile ~another/.ssh/key",
        ] {
            let snapshot = parsed(&format!("Host target\n {option}\n"));
            assert!(!snapshot.entries[0].blocked.is_empty(), "{option}");
        }
    }

    #[test]
    fn supported_identities_only_with_a_user_and_key_has_no_warnings() {
        let snapshot = parsed(
            "Host target\n User fixture\n IdentityFile /keys/selected\n IdentitiesOnly yes\n",
        );
        let entry = &snapshot.entries[0];
        assert!(entry.warnings.is_empty());
        assert!(entry.blocked.is_empty());
        assert!(matches!(
            &entry.profile.auth,
            AuthConfig::Key { key_path, passphrase: None } if key_path == "/keys/selected"
        ));
    }

    #[test]
    fn explicit_none_and_missing_identity_never_select_agent_implicitly() {
        let snapshot = parsed(
            "Host missing\nHost none\n IdentityFile none\n IdentitiesOnly yes\nHost multiple\n IdentityFile /keys/one\n IdentityFile /keys/two\nHost single\n IdentityFile none\n IdentityFile ~/.ssh/key\n IdentitiesOnly yes\n",
        );
        assert!(matches!(
            snapshot.entries[0].profile.auth,
            AuthConfig::Password { password: None }
        ));
        for index in [1, 2] {
            assert!(matches!(
                &snapshot.entries[index].profile.auth,
                AuthConfig::Key { key_path, passphrase: None } if key_path.is_empty()
            ));
        }
        assert!(snapshot.entries[1].identity_files.is_empty());
        assert!(matches!(
            &snapshot.entries[3].profile.auth,
            AuthConfig::Key { key_path, passphrase: None } if key_path == "/fixture/home/.ssh/key"
        ));
        assert!(snapshot
            .entries
            .iter()
            .all(|entry| entry.blocked.is_empty()));
    }

    #[test]
    fn malformed_syntax_fails_without_echoing_source_values() {
        for text in [
            "Host",
            "Host target\n User",
            "Host target\n IdentityFile \"secret-unclosed",
            "Host target\n User secret-trailing\\",
            "Host !",
            "=secret-value",
        ] {
            let error = parse(text, &source()).unwrap_err();
            assert!(error.contains("行"));
            assert!(!error.contains("secret"));
        }
    }

    #[test]
    fn missing_and_non_file_sources_report_errors_but_empty_files_are_valid() {
        let directory = tempfile::tempdir().unwrap();
        let mut source = SshConfigSource {
            path: directory.path().join("missing"),
            home: directory.path().to_owned(),
        };
        assert!(source.load().unwrap_err().contains("无法读取"));
        fs::write(&source.path, "").unwrap();
        assert!(source.load().unwrap().entries.is_empty());
        source.path = directory.path().to_owned();
        assert!(source.load().unwrap_err().contains("无法读取"));
    }

    #[test]
    fn invalid_utf8_reports_source_failure() {
        let directory = tempfile::tempdir().unwrap();
        let source = SshConfigSource {
            path: directory.path().join("config"),
            home: directory.path().to_owned(),
        };
        fs::write(&source.path, [0xff, 0xfe]).unwrap();
        assert!(source.load().is_err());
    }
}
