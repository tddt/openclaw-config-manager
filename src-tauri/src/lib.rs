use std::fs;
use std::path::PathBuf;

#[tauri::command]
fn get_config_path() -> Result<String, String> {
    let home = dirs_home().ok_or("Cannot determine home directory")?;
    let config_path = home.join(".openclaw").join("openclaw.json");
    Ok(config_path.to_string_lossy().to_string())
}

#[tauri::command]
fn read_config_file(path: String) -> Result<String, String> {
    fs::read_to_string(&path).map_err(|e| format!("Failed to read {}: {}", path, e))
}

#[tauri::command]
fn write_config_file(path: String, content: String) -> Result<(), String> {
    // Create backup before writing
    let backup_path = format!("{}.bak", path);
    if let Ok(existing) = fs::read_to_string(&path) {
        let _ = fs::write(&backup_path, &existing);
    }
    fs::write(&path, content).map_err(|e| format!("Failed to write {}: {}", path, e))
}

#[tauri::command]
fn read_cron_jobs(base_dir: String) -> Result<String, String> {
    let path = PathBuf::from(&base_dir).join("cron").join("jobs.json");
    if path.exists() {
        fs::read_to_string(&path).map_err(|e| e.to_string())
    } else {
        Ok(r#"{"jobs":[]}"#.to_string())
    }
}

#[tauri::command]
fn write_cron_jobs(base_dir: String, content: String) -> Result<(), String> {
    let dir = PathBuf::from(&base_dir).join("cron");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("jobs.json");
    fs::write(&path, content).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_skills(base_dir: String) -> Result<Vec<serde_json::Value>, String> {
    let skills_dir = PathBuf::from(&base_dir).join("skills");
    let mut skills = Vec::new();
    if let Ok(entries) = fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let content = fs::read_to_string(&skill_md).unwrap_or_default();
                    skills.push(serde_json::json!({
                        "name": name,
                        "path": path.to_string_lossy().to_string(),
                        "content": content
                    }));
                }
            }
        }
    }
    Ok(skills)
}

#[tauri::command]
fn get_openclaw_home() -> Result<String, String> {
    let home = dirs_home().ok_or("Cannot determine home directory")?;
    Ok(home.join(".openclaw").to_string_lossy().to_string())
}

/// Check if the gateway TCP port is open (bypasses CORS).
#[tauri::command]
fn check_gateway_port(port: u16) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    match format!("127.0.0.1:{}", port).parse::<std::net::SocketAddr>() {
        Ok(addr) => TcpStream::connect_timeout(&addr, Duration::from_millis(2000)).is_ok(),
        Err(_) => false,
    }
}

/// Read any text file from disk (for agent markdown files).
#[tauri::command]
async fn read_text_file(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        fs::read_to_string(&path).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string())?
}

/// Write any text file to disk (for agent markdown files).
#[tauri::command]
async fn write_text_file(path: String, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, content).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string())?
}

/// HTTP POST with JSON body — async wrapper to avoid blocking the main thread.
#[tauri::command]
async fn http_post_json(url: String, token: Option<String>, body: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        http_post_blocking(url, token, body)
    }).await.map_err(|e| format!("spawn error: {}", e))?
}

fn http_post_blocking(url: String, token: Option<String>, body: String) -> Result<String, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let rest = url.strip_prefix("http://").ok_or("仅支持 http:// 协议")?;
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };

    let addr: std::net::SocketAddr = host_port
        .parse()
        .map_err(|_| format!("无效地址: {}", host_port))?;

    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("连接 Gateway ({}) 失败: {}", host_port, e))?;
    stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();

    let auth_line = match &token {
        Some(t) if !t.is_empty() => format!("Authorization: Bearer {}\r\n", t),
        _ => String::new(),
    };
    let body_bytes = body.as_bytes();
    let req_header = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
        path, host_port, body_bytes.len(), auth_line
    );
    stream.write_all(req_header.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(body_bytes).map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);

    // Status line
    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|e| e.to_string())?;
    let status: u16 = status_line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);

    // Headers
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if line.trim().is_empty() { break; }
        let low = line.to_ascii_lowercase();
        if low.starts_with("content-length:") {
            content_length = low["content-length:".len()..].trim().parse().ok();
        }
        if low.contains("transfer-encoding: chunked") { chunked = true; }
    }

    // Body
    let resp_body = if chunked {
        let mut out = String::new();
        loop {
            let mut sz = String::new();
            reader.read_line(&mut sz).map_err(|e| e.to_string())?;
            let n = usize::from_str_radix(sz.trim(), 16).unwrap_or(0);
            if n == 0 { break; }
            let mut buf = vec![0u8; n];
            { use std::io::Read; reader.read_exact(&mut buf).map_err(|e| e.to_string())?; }
            out.push_str(&String::from_utf8_lossy(&buf));
            let mut crlf = String::new();
            reader.read_line(&mut crlf).ok();
        }
        out
    } else if let Some(len) = content_length {
        let mut buf = vec![0u8; len];
        { use std::io::Read; reader.read_exact(&mut buf).map_err(|e| e.to_string())?; }
        String::from_utf8_lossy(&buf).to_string()
    } else {
        let mut s = String::new();
        { use std::io::Read; reader.read_to_string(&mut s).map_err(|e| e.to_string())?; }
        s
    };

    if status >= 400 {
        return Err(format!("HTTP {}: {}", status, resp_body));
    }
    Ok(resp_body)
}

/// Invoke an agent turn via the openclaw CLI.
/// Returns the full JSON output from `openclaw agent --agent <id> --message <text> --json`.
#[tauri::command]
async fn invoke_agent(agent_id: String, message: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        invoke_agent_blocking(&agent_id, &message)
    }).await.map_err(|e| format!("spawn error: {}", e))?
}

fn invoke_agent_blocking(agent_id: &str, message: &str) -> Result<String, String> {
    use std::process::Command;

    // Find the openclaw executable in PATH
    let openclaw_bin = find_openclaw_bin();

    let output = Command::new(&openclaw_bin)
        .args(["agent", "--agent", agent_id, "--message", message, "--json"])
        .output()
        .map_err(|e| format!("无法启动 openclaw: {} (路径: {})", e, openclaw_bin))?;

    // On Windows the process output may be in GBK/CP936 — decode bytes manually.
    let stdout = decode_windows_output(&output.stdout);
    let stderr = decode_windows_output(&output.stderr);

    if !output.status.success() {
        // Try to extract a useful message from stderr/stdout
        let detail = if stderr.trim().is_empty() { stdout.trim().to_string() } else { stderr.trim().to_string() };
        return Err(format!("openclaw 返回非零退出码 {:?}: {}", output.status.code(), detail));
    }

    // The stdout may contain warning lines before the JSON object.
    // Find the first line that starts with '{' or '['.
    let _json_first_line = stdout
        .lines()
        .find(|line| {
            let t = line.trim();
            t.starts_with('{') || t.starts_with('[')
        })
        .ok_or_else(|| format!("openclaw 未返回 JSON。输出: {}", stdout))?;

    // Return just that JSON line (the rest of the output is buffered warnings).
    // If the response spans multiple lines we need to collect from the first '{' to end.
    let json_start = stdout.find(|c| c == '{' || c == '[').unwrap_or(0);
    Ok(stdout[json_start..].trim().to_string())
}

#[tauri::command]
fn get_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn find_openclaw_bin() -> String {
    // On Windows, try npm global install locations via APPDATA.
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            std::env::var("APPDATA")
                .map(|d| format!("{}/npm/openclaw.cmd", d))
                .unwrap_or_default(),
            std::env::var("APPDATA")
                .map(|d| format!("{}/npm/openclaw", d))
                .unwrap_or_default(),
        ];
        for c in &candidates {
            if !c.is_empty() && std::path::Path::new(c).exists() {
                return c.clone();
            }
        }
    }
    // On macOS/Linux, check static system paths then HOME-relative install paths.
    #[cfg(not(target_os = "windows"))]
    {
        let static_candidates: &[&str] = &[
            "/opt/homebrew/bin/openclaw", // macOS ARM Homebrew
            "/usr/local/bin/openclaw",    // macOS Intel Homebrew / npm global
            "/usr/bin/openclaw",          // Linux system install
        ];
        for c in static_candidates {
            if std::path::Path::new(c).exists() {
                return c.to_string();
            }
        }
        // HOME-relative: pnpm, npm user-global, nix, cargo
        if let Ok(home) = std::env::var("HOME") {
            let home_candidates = [
                format!("{}/Library/pnpm/openclaw", home),       // macOS pnpm
                format!("{}/.local/share/pnpm/openclaw", home),  // Linux pnpm
                format!("{}/.npm-global/bin/openclaw", home),    // npm user-global
                format!("{}/.local/bin/openclaw", home),         // user local bin
                format!("{}/.cargo/bin/openclaw", home),         // cargo install
            ];
            for c in &home_candidates {
                if std::path::Path::new(c).exists() {
                    return c.clone();
                }
            }
        }
    }
    // Fallback: rely on PATH
    "openclaw".to_string()
}

/// Attempt to decode bytes as UTF-8; fall back to lossy if needed.
/// On Windows, some CLI tools write CP936/GBK. We try UTF-8 first (handles most cases),
/// then fall back to lossy UTF-8 replacement.
fn decode_windows_output(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            // Try Windows CP936 / GBK via byte-by-byte fallback
            #[cfg(target_os = "windows")]
            {
                decode_gbk(bytes)
            }
            #[cfg(not(target_os = "windows"))]
            {
                String::from_utf8_lossy(bytes).to_string()
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn decode_gbk(bytes: &[u8]) -> String {
    // Use Windows MultiByteToWideChar API to decode CP936
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    extern "system" {
        fn MultiByteToWideChar(
            code_page: u32,
            dw_flags: u32,
            lp_multi_byte_str: *const u8,
            cb_multi_byte: i32,
            lp_wide_char_str: *mut u16,
            cch_wide_char: i32,
        ) -> i32;
    }

    const CP_ACP: u32 = 0; // system default ANSI code page
    if bytes.is_empty() {
        return String::new();
    }
    unsafe {
        let needed = MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if needed <= 0 { return String::from_utf8_lossy(bytes).to_string(); }
        let mut wide: Vec<u16> = vec![0u16; needed as usize];
        let written = MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), needed);
        if written <= 0 { return String::from_utf8_lossy(bytes).to_string(); }
        OsString::from_wide(&wide[..written as usize]).to_string_lossy().to_string()
    }
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var("HOMEDRIVE").ok()?;
                let path = std::env::var("HOMEPATH").ok()?;
                Some(PathBuf::from(format!("{}{}", drive, path)))
            })
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_config_path,
            read_config_file,
            write_config_file,
            read_cron_jobs,
            write_cron_jobs,
            list_skills,
            get_openclaw_home,
            get_platform,
            check_gateway_port,
            read_text_file,
            write_text_file,
            http_post_json,
            invoke_agent,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
