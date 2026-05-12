use agent_notify_core::SessionInfo;
use std::time::Duration;

#[allow(dead_code)]
pub fn focus_window(session: &SessionInfo) -> bool {
    focus_window_with_logger(session, |_| {})
}

#[allow(dead_code)]
pub fn focus_window_handle(session: &SessionInfo) -> bool {
    focus_window_handle_with_logger(session, |_| {})
}

pub fn focus_window_with_logger<F>(session: &SessionInfo, mut log: F) -> bool
where
    F: FnMut(&str),
{
    #[cfg(windows)]
    {
        log(&format!(
            "code=session session={} hwnd={} window_pid={} pid={} parent_pid={} title={}",
            sanitize_log_text(&session.session_id),
            session
                .window
                .as_ref()
                .and_then(|window| window.hwnd)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            session
                .window
                .as_ref()
                .and_then(|window| window.pid)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            session
                .process
                .as_ref()
                .and_then(|process| process.pid)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            session
                .process
                .as_ref()
                .and_then(|process| process.parent_pid)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            session
                .window
                .as_ref()
                .and_then(|window| window.title.as_deref())
                .map(sanitize_log_text)
                .unwrap_or_else(|| "<none>".to_string())
        ));
        if let Some(hwnd) = session.window.as_ref().and_then(|window| window.hwnd)
            && focus_validated_hwnd(hwnd, session, &mut log)
        {
            log("code=focus_success method=hwnd");
            return true;
        }
        if let Some(pid) = session.process.as_ref().and_then(|process| process.pid)
            && focus_pid(pid, &mut log)
        {
            log("code=focus_success method=pid");
            return true;
        }
        if let Some(parent_pid) = session
            .process
            .as_ref()
            .and_then(|process| process.parent_pid)
            && focus_pid(parent_pid, &mut log)
        {
            log("code=focus_success method=parent_pid");
            return true;
        }
        if let Some(title) = session
            .window
            .as_ref()
            .and_then(|window| window.title.as_deref())
            && focus_by_title(title, &mut log)
        {
            log("code=focus_success method=title");
            return true;
        }
        if let Some(window_pid) = session.window.as_ref().and_then(|window| window.pid)
            && focus_pid(window_pid, &mut log)
        {
            log("code=focus_success method=window_pid");
            return true;
        }
        log("code=focus_failed");
    }
    #[cfg(not(windows))]
    {
        let _ = session;
        let _ = &mut log;
    }
    false
}

pub fn focus_window_handle_with_logger<F>(session: &SessionInfo, mut log: F) -> bool
where
    F: FnMut(&str),
{
    #[cfg(windows)]
    {
        log_session(session, &mut log);
        if let Some(hwnd) = session.window.as_ref().and_then(|window| window.hwnd)
            && focus_validated_hwnd(hwnd, session, &mut log)
        {
            log("code=focus_success method=shared_hwnd");
            return true;
        }
        log("code=focus_failed method=shared_hwnd");
    }
    #[cfg(not(windows))]
    {
        let _ = session;
        let _ = &mut log;
    }
    false
}

#[cfg(windows)]
fn log_session<F>(session: &SessionInfo, log: &mut F)
where
    F: FnMut(&str),
{
    log(&format!(
        "code=session session={} hwnd={} window_pid={} pid={} parent_pid={} title={}",
        sanitize_log_text(&session.session_id),
        session
            .window
            .as_ref()
            .and_then(|window| window.hwnd)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        session
            .window
            .as_ref()
            .and_then(|window| window.pid)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        session
            .process
            .as_ref()
            .and_then(|process| process.pid)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        session
            .process
            .as_ref()
            .and_then(|process| process.parent_pid)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        session
            .window
            .as_ref()
            .and_then(|window| window.title.as_deref())
            .map(sanitize_log_text)
            .unwrap_or_else(|| "<none>".to_string())
    ));
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowIdentity {
    pid: u32,
    title: String,
}

#[derive(Debug, Clone)]
struct WindowProbe {
    identity: WindowIdentity,
    host_image: Option<String>,
    class_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityMatch {
    L1Pid,
    L2HostImage,
    L3HostClass,
    L4Title,
    L5Empty,
    Reject,
}

impl IdentityMatch {
    fn tier_label(self) -> &'static str {
        match self {
            IdentityMatch::L1Pid => "L1Pid",
            IdentityMatch::L2HostImage => "L2HostImage",
            IdentityMatch::L3HostClass => "L3HostClass",
            IdentityMatch::L4Title => "L4Title",
            IdentityMatch::L5Empty => "L5Empty",
            IdentityMatch::Reject => "Reject",
        }
    }

    fn allows_focus(self) -> bool {
        !matches!(self, IdentityMatch::Reject)
    }
}

fn is_known_terminal_image(name: &str) -> bool {
    matches!(
        name,
        "conhost.exe"
            | "openconsole.exe"
            | "windowsterminal.exe"
            | "wt.exe"
            | "alacritty.exe"
            | "mintty.exe"
            | "wezterm-gui.exe"
            | "tabby.exe"
            | "conemu64.exe"
            | "conemu.exe"
            | "conemuc64.exe"
            | "conemuc.exe"
    )
}

fn is_known_terminal_class(name: &str) -> bool {
    matches!(
        name,
        "ConsoleWindowClass"
            | "CASCADIA_HOSTING_WINDOW_CLASS"
            | "PseudoConsoleWindow"
            | "VirtualConsoleClass"
            | "mintty"
            | "Alacritty"
            | "org.wezfurlong.wezterm"
    )
}

fn identity_matches_session(probe: &WindowProbe, session: &SessionInfo) -> IdentityMatch {
    let expected_pids = [
        session.window.as_ref().and_then(|window| window.pid),
        session.process.as_ref().and_then(|process| process.pid),
        session
            .process
            .as_ref()
            .and_then(|process| process.parent_pid),
    ];
    let has_expected_pid = expected_pids.iter().any(Option::is_some);
    if expected_pids
        .iter()
        .flatten()
        .any(|expected| *expected != 0 && *expected == probe.identity.pid)
    {
        return IdentityMatch::L1Pid;
    }

    if probe
        .host_image
        .as_deref()
        .is_some_and(is_known_terminal_image)
    {
        return IdentityMatch::L2HostImage;
    }

    if probe
        .class_name
        .as_deref()
        .is_some_and(is_known_terminal_class)
    {
        return IdentityMatch::L3HostClass;
    }

    let expected_title = session
        .window
        .as_ref()
        .and_then(|window| window.title.as_deref())
        .map(str::trim)
        .filter(|title| !title.is_empty());
    if expected_title.is_some_and(|expected| expected == probe.identity.title.trim()) {
        return IdentityMatch::L4Title;
    }

    if !has_expected_pid && expected_title.is_none() {
        IdentityMatch::L5Empty
    } else {
        IdentityMatch::Reject
    }
}

#[cfg(windows)]
fn focus_validated_hwnd<F>(hwnd: isize, session: &SessionInfo, log: &mut F) -> bool
where
    F: FnMut(&str),
{
    let Some(candidate) = window_candidate(hwnd, log) else {
        return false;
    };
    let probe = WindowProbe {
        identity: candidate.identity.clone(),
        host_image: candidate.host_image.clone(),
        class_name: candidate.class_name.clone(),
    };
    let verdict = identity_matches_session(&probe, session);
    if !verdict.allows_focus() {
        log(&format!(
            "code=focus_hwnd_mismatch hwnd={} actual_pid={} actual_title={} actual_image={} actual_class={} expected_window_pid={} expected_pid={} expected_parent_pid={} expected_title={}",
            candidate.hwnd as isize,
            candidate.identity.pid,
            sanitize_log_text(&candidate.identity.title),
            candidate
                .host_image
                .as_deref()
                .unwrap_or("<none>"),
            candidate
                .class_name
                .as_deref()
                .unwrap_or("<none>"),
            session
                .window
                .as_ref()
                .and_then(|window| window.pid)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            session
                .process
                .as_ref()
                .and_then(|process| process.pid)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            session
                .process
                .as_ref()
                .and_then(|process| process.parent_pid)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            session
                .window
                .as_ref()
                .and_then(|window| window.title.as_deref())
                .map(sanitize_log_text)
                .unwrap_or_else(|| "<none>".to_string()),
        ));
        return false;
    }
    log(&format!(
        "code=focus_hwnd_allow tier={} hwnd={} actual_pid={} actual_image={} actual_class={}",
        verdict.tier_label(),
        candidate.hwnd as isize,
        candidate.identity.pid,
        candidate.host_image.as_deref().unwrap_or("<none>"),
        candidate.class_name.as_deref().unwrap_or("<none>"),
    ));
    focus_hwnd(candidate.hwnd as isize, log)
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct WindowCandidate {
    hwnd: windows_sys::Win32::Foundation::HWND,
    identity: WindowIdentity,
    host_image: Option<String>,
    class_name: Option<String>,
}

#[cfg(windows)]
unsafe fn query_process_image_basename(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut len: u32 = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 || len == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buffer[..len as usize]);
        std::path::Path::new(&full)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_ascii_lowercase())
    }
}

#[cfg(windows)]
unsafe fn read_window_class_name(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW;

    unsafe {
        let mut buffer = [0u16; 256];
        let copied = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if copied <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..copied as usize]))
    }
}

#[cfg(windows)]
fn is_promotable_pseudoconsole_class(name: &str) -> bool {
    matches!(name, "PseudoConsoleWindow" | "VirtualConsoleClass")
}

#[cfg(windows)]
fn window_candidate<F>(hwnd: isize, log: &mut F) -> Option<WindowCandidate>
where
    F: FnMut(&str),
{
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GA_ROOTOWNER, GetAncestor, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    };

    unsafe {
        let mut hwnd = hwnd as HWND;
        if hwnd.is_null() {
            log("code=focus_hwnd_skip hwnd=0 is_window=false is_visible=false");
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        if !root.is_null() {
            hwnd = root;
        }
        let initial_class = read_window_class_name(hwnd);
        let promoted = if let Some(class) = initial_class.as_deref()
            && is_promotable_pseudoconsole_class(class)
        {
            let owner = GetAncestor(hwnd, GA_ROOTOWNER);
            if !owner.is_null() && owner != hwnd && IsWindow(owner) != 0 {
                log(&format!(
                    "code=focus_hwnd_promote_owner hwnd={} owner={} from_class={}",
                    hwnd as isize, owner as isize, class
                ));
                hwnd = owner;
                true
            } else {
                false
            }
        } else {
            false
        };
        let is_window = IsWindow(hwnd) != 0;
        let is_visible = IsWindowVisible(hwnd) != 0;
        if !is_window || !is_visible {
            log(&format!(
                "code=focus_hwnd_skip hwnd={} is_window={} is_visible={}",
                hwnd as isize, is_window, is_visible
            ));
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let length = GetWindowTextLengthW(hwnd);
        let title = if length > 0 {
            let mut buffer = vec![0u16; length as usize + 1];
            let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
            if copied > 0 {
                String::from_utf16_lossy(&buffer[..copied as usize])
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        let host_image = query_process_image_basename(pid);
        let class_name = if promoted {
            read_window_class_name(hwnd)
        } else {
            initial_class
        };
        Some(WindowCandidate {
            hwnd,
            identity: WindowIdentity { pid, title },
            host_image,
            class_name,
        })
    }
}

#[cfg(windows)]
fn focus_hwnd<F>(hwnd: isize, log: &mut F) -> bool
where
    F: FnMut(&str),
{
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SW_RESTORE,
        SetForegroundWindow, ShowWindow, ShowWindowAsync,
    };

    unsafe {
        let Some(candidate) = window_candidate(hwnd, log) else {
            return false;
        };
        let hwnd = candidate.hwnd;
        let foreground_before = GetForegroundWindow();
        let current_thread = GetCurrentThreadId();
        let mut target_pid = 0;
        let target_thread = GetWindowThreadProcessId(hwnd, &mut target_pid);
        let foreground_thread = if foreground_before.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground_before, std::ptr::null_mut())
        };
        ShowWindow(hwnd, SW_RESTORE);
        let direct = SetForegroundWindow(hwnd) != 0;
        std::thread::sleep(Duration::from_millis(80));
        let foreground_after_direct = GetForegroundWindow();
        if foreground_matches(hwnd, foreground_after_direct) {
            log(&format!(
                "code=focus_hwnd_result hwnd={} direct={} fallback=false foreground_before={} foreground_after={} current_thread={} target_thread={} target_pid={} foreground_thread={} success=true",
                hwnd as isize,
                direct,
                foreground_before as isize,
                foreground_after_direct as isize,
                current_thread,
                target_thread,
                target_pid,
                foreground_thread
            ));
            return true;
        }

        let attached_foreground = foreground_thread != 0
            && foreground_thread != current_thread
            && AttachThreadInput(current_thread, foreground_thread, 1) != 0;
        let attached_target = target_thread != 0
            && target_thread != current_thread
            && AttachThreadInput(current_thread, target_thread, 1) != 0;
        ShowWindowAsync(hwnd, SW_RESTORE);
        let brought = BringWindowToTop(hwnd) != 0;
        let active = !SetActiveWindow(hwnd).is_null();
        let focused = !SetFocus(hwnd).is_null();
        let fallback = SetForegroundWindow(hwnd) != 0;
        std::thread::sleep(Duration::from_millis(120));
        let foreground_after = GetForegroundWindow();
        if attached_target {
            AttachThreadInput(current_thread, target_thread, 0);
        }
        if attached_foreground {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
        let success = foreground_matches(hwnd, foreground_after);
        log(&format!(
            "code=focus_hwnd_result hwnd={} direct={} attached_foreground={} attached_target={} brought={} active={} focused={} fallback={} foreground_before={} foreground_after={} current_thread={} target_thread={} target_pid={} foreground_thread={} success={}",
            hwnd as isize,
            direct,
            attached_foreground,
            attached_target,
            brought,
            active,
            focused,
            fallback,
            foreground_before as isize,
            foreground_after as isize,
            current_thread,
            target_thread,
            target_pid,
            foreground_thread,
            success
        ));
        success
    }
}

#[cfg(windows)]
unsafe fn foreground_matches(
    target: windows_sys::Win32::Foundation::HWND,
    foreground: windows_sys::Win32::Foundation::HWND,
) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor};

    if target.is_null() || foreground.is_null() {
        return false;
    }
    if foreground == target {
        return true;
    }
    let foreground_root = unsafe { GetAncestor(foreground, GA_ROOT) };
    !foreground_root.is_null() && foreground_root == target
}

#[cfg(windows)]
fn focus_pid<F>(pid: u32, log: &mut F) -> bool
where
    F: FnMut(&str),
{
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;

    struct Search {
        pid: u32,
        matches: Vec<HWND>,
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowThreadProcessId, IsWindowVisible,
        };

        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut window_pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut window_pid);
        }
        if window_pid == search.pid {
            search.matches.push(hwnd);
        }
        1
    }

    if pid == 0 {
        return false;
    }
    let mut search = Search {
        pid,
        matches: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(enum_window), &mut search as *mut Search as LPARAM);
    }
    if search.matches.len() != 1 {
        log(&format!(
            "code=focus_pid_miss pid={pid} matches={}",
            search.matches.len()
        ));
        return false;
    }
    log(&format!(
        "code=focus_pid_match pid={} hwnd={}",
        pid, search.matches[0] as isize
    ));
    focus_hwnd(search.matches[0] as isize, log)
}

#[cfg(windows)]
fn focus_by_title<F>(title: &str, log: &mut F) -> bool
where
    F: FnMut(&str),
{
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;

    struct Match {
        hwnd: HWND,
        class_name: Option<String>,
        title: String,
    }

    struct Search {
        title: String,
        terminal: Vec<Match>,
        generic: Vec<Match>,
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
        };

        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let length = unsafe { GetWindowTextLengthW(hwnd) };
        if length <= 0 {
            return 1;
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        if copied <= 0 {
            return 1;
        }
        let window_title = String::from_utf16_lossy(&buffer[..copied as usize]);
        let search = unsafe { &mut *(lparam as *mut Search) };
        if window_title.trim() != search.title.trim() {
            return 1;
        }
        let class_name = unsafe { read_window_class_name(hwnd) };
        let is_terminal = class_name
            .as_deref()
            .is_some_and(is_known_terminal_class);
        let entry = Match {
            hwnd,
            class_name,
            title: window_title,
        };
        if is_terminal {
            search.terminal.push(entry);
        } else {
            search.generic.push(entry);
        }
        1
    }

    let title = title.trim();
    if title.is_empty() {
        return false;
    }
    let mut search = Search {
        title: title.to_string(),
        terminal: Vec::new(),
        generic: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(enum_window), &mut search as *mut Search as LPARAM);
    }
    if search.terminal.len() == 1 {
        let entry = search.terminal.remove(0);
        log(&format!(
            "code=focus_terminal_title_match hwnd={} class={} title={}",
            entry.hwnd as isize,
            sanitize_log_text(entry.class_name.as_deref().unwrap_or("<none>")),
            sanitize_log_text(&entry.title),
        ));
        return focus_hwnd(entry.hwnd as isize, log);
    }
    if !search.terminal.is_empty() {
        log(&format!(
            "code=focus_terminal_title_miss matches={} title={}",
            search.terminal.len(),
            sanitize_log_text(title)
        ));
    }
    if search.generic.len() == 1 {
        let entry = search.generic.remove(0);
        log(&format!(
            "code=focus_title_match hwnd={} title={}",
            entry.hwnd as isize,
            sanitize_log_text(&entry.title)
        ));
        return focus_hwnd(entry.hwnd as isize, log);
    }
    log(&format!(
        "code=focus_title_miss matches={} title={}",
        search.generic.len(),
        sanitize_log_text(title)
    ));
    false
}

fn sanitize_log_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' => ' ',
            _ => ch,
        })
        .take(480)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_notify_core::{
        EventType, MessageInfo, ProcessInfo, ProjectInfo, SessionStatus, WindowInfo,
    };

    fn session(
        window_pid: Option<u32>,
        pid: Option<u32>,
        parent_pid: Option<u32>,
        title: &str,
    ) -> SessionInfo {
        SessionInfo {
            session_id: "codex-test".to_string(),
            tool: "codex".to_string(),
            project: ProjectInfo {
                cwd: r"D:\repo".to_string(),
                name: "repo".to_string(),
            },
            status: SessionStatus::Completed,
            last_event_type: EventType::TaskCompleted,
            last_message: MessageInfo {
                title: "done".to_string(),
                body: "done".to_string(),
                detail: None,
            },
            process: Some(ProcessInfo {
                pid,
                parent_pid,
                started_at: None,
            }),
            window: Some(WindowInfo {
                pid: window_pid,
                title: Some(title.to_string()),
                hwnd: Some(100),
                terminal: Some("WindowsTerminal".to_string()),
            }),
            updated_at: "2026-05-11T00:00:00Z".to_string(),
        }
    }

    fn probe(
        pid: u32,
        title: &str,
        host_image: Option<&str>,
        class_name: Option<&str>,
    ) -> WindowProbe {
        WindowProbe {
            identity: WindowIdentity {
                pid,
                title: title.to_string(),
            },
            host_image: host_image.map(str::to_string),
            class_name: class_name.map(str::to_string),
        }
    }

    #[test]
    fn l1_pid_match_wins_over_lower_tiers() {
        let session = session(Some(42), Some(10), Some(11), "repo - codex");
        let probe = probe(42, "anything", Some("chrome.exe"), Some("Chrome_WidgetWin_1"));
        assert_eq!(
            identity_matches_session(&probe, &session),
            IdentityMatch::L1Pid
        );
    }

    #[test]
    fn l2_host_image_allows_mismatched_pid() {
        let session = session(Some(42), Some(10), Some(11), "old title");
        let probe = probe(9999, "new title", Some("conhost.exe"), Some("ConsoleWindowClass"));
        assert_eq!(
            identity_matches_session(&probe, &session),
            IdentityMatch::L2HostImage
        );
    }

    #[test]
    fn l3_host_class_allows_when_image_unobtainable() {
        let session = session(Some(42), Some(10), Some(11), "old title");
        let probe = probe(9999, "new title", None, Some("CASCADIA_HOSTING_WINDOW_CLASS"));
        assert_eq!(
            identity_matches_session(&probe, &session),
            IdentityMatch::L3HostClass
        );
    }

    #[test]
    fn rejects_non_terminal_window_when_expected_data_exists() {
        let session = session(Some(42), Some(10), Some(11), "repo - codex");
        let probe = probe(
            99,
            "Agent Notify",
            Some("chrome.exe"),
            Some("Chrome_WidgetWin_1"),
        );
        assert_eq!(
            identity_matches_session(&probe, &session),
            IdentityMatch::Reject
        );
    }

    #[test]
    fn l4_title_match_for_unknown_terminal_host() {
        let session = session(Some(42), Some(10), Some(11), "repo - codex");
        let probe = probe(99, "repo - codex", Some("unknown_term.exe"), None);
        assert_eq!(
            identity_matches_session(&probe, &session),
            IdentityMatch::L4Title
        );
    }

    #[test]
    fn l5_empty_when_no_expected_data() {
        let mut session = session(None, None, None, "");
        if let Some(window) = session.window.as_mut() {
            window.title = None;
        }
        let probe = probe(99, "anything", Some("unknown.exe"), Some("UnknownClass"));
        assert_eq!(
            identity_matches_session(&probe, &session),
            IdentityMatch::L5Empty
        );
    }

    #[test]
    fn terminal_image_allowlist_is_lowercase_only() {
        assert!(is_known_terminal_image("conhost.exe"));
        assert!(is_known_terminal_image("windowsterminal.exe"));
        assert!(!is_known_terminal_image("Conhost.EXE"));
        assert!(!is_known_terminal_image("notepad.exe"));
    }

    #[test]
    fn focus_verdict_allows_focus_for_all_match_tiers() {
        for verdict in [
            IdentityMatch::L1Pid,
            IdentityMatch::L2HostImage,
            IdentityMatch::L3HostClass,
            IdentityMatch::L4Title,
            IdentityMatch::L5Empty,
        ] {
            assert!(verdict.allows_focus(), "tier {:?} must allow focus", verdict);
        }
        assert!(!IdentityMatch::Reject.allows_focus());
    }

    #[cfg(windows)]
    #[test]
    fn foreground_match_rejects_null_handles() {
        unsafe {
            assert!(!foreground_matches(
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ));
        }
    }
}
