//! How the OS is scheduling the calling thread, for setup diagnostics.
//!
//! A session's setup runs on that session's own thread, so a thread the OS
//! has throttled cannot read the agent's answer or fire the setup timer. On
//! macOS that throttling (App Nap, a backgrounded task, a clamped QoS) shows
//! as a darwin-background flag and a scheduling priority of 4 instead of 31.
//! Snapshotting it at setup start and when the setup timer fires makes that
//! visible in the log and the error.
//!
//! Every query is independent and best effort: one that fails is left out,
//! and platforms without these APIs get no snapshot at all.

use std::fmt;

/// One snapshot of the calling thread's scheduling state. Fields the OS
/// would not report are `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SchedState {
    /// The process is darwin-background (`PRIO_DARWIN_PROCESS`).
    pub process_background: Option<bool>,
    /// The thread is darwin-background (`PRIO_DARWIN_THREAD`).
    pub thread_background: Option<bool>,
    /// The thread's QoS class, as `pthread_get_qos_class_np` names it.
    pub thread_qos: Option<&'static str>,
    /// The thread's current scheduling priority (what `ps -M` shows as PRI).
    pub thread_priority: Option<i32>,
    /// The thread's base scheduling priority.
    pub thread_base_priority: Option<i32>,
    /// The task's role (`TASK_CATEGORY_POLICY`).
    pub task_role: Option<&'static str>,
    /// The 1, 5 and 15 minute load averages.
    pub load_average: Option<[f64; 3]>,
}

impl SchedState {
    /// Whether anything marks the thread as throttled.
    fn background(&self) -> bool {
        self.process_background == Some(true) || self.thread_background == Some(true)
    }
}

impl fmt::Display for SchedState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        match (self.thread_priority, self.thread_base_priority) {
            (Some(current), Some(base)) => parts.push(format!("pri {current}/{base}")),
            (Some(current), None) => parts.push(format!("pri {current}")),
            _ => {}
        }
        if self.background() {
            let mut which = Vec::new();
            if self.process_background == Some(true) {
                which.push("process");
            }
            if self.thread_background == Some(true) {
                which.push("thread");
            }
            parts.push(format!("darwin-bg {}", which.join("+")));
        }
        if let Some(qos) = self.thread_qos {
            parts.push(format!("qos {qos}"));
        }
        if let Some(role) = self.task_role {
            parts.push(format!("role {role}"));
        }
        if let Some([one, five, fifteen]) = self.load_average {
            parts.push(format!("load {one:.1}/{five:.1}/{fifteen:.1}"));
        }
        if parts.is_empty() {
            f.write_str("unavailable")
        } else {
            f.write_str(&parts.join(", "))
        }
    }
}

/// Snapshot the calling thread's scheduling state.
pub(crate) fn current() -> SchedState {
    platform::current()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::SchedState;

    /// `TASK_CATEGORY_POLICY` from `<mach/task_policy.h>`.
    const TASK_CATEGORY_POLICY: u32 = 1;

    extern "C" {
        // Declared here rather than taken from `libc`: `libc` types the class
        // as a Rust enum, and reading a value it does not list would be UB.
        fn pthread_get_qos_class_np(
            thread: libc::pthread_t,
            class: *mut u32,
            relative_priority: *mut libc::c_int,
        ) -> libc::c_int;
        /// What `mach_task_self()` reads; `libc`'s wrapper is deprecated.
        static mach_task_self_: libc::mach_port_t;
        fn task_policy_get(
            task: libc::mach_port_t,
            flavor: u32,
            policy_info: *mut libc::integer_t,
            count: *mut libc::mach_msg_type_number_t,
            get_default: *mut libc::boolean_t,
        ) -> libc::kern_return_t;
    }

    pub(super) fn current() -> SchedState {
        let (thread_priority, thread_base_priority) = thread_priorities();
        SchedState {
            process_background: darwin_background(libc::PRIO_DARWIN_PROCESS),
            thread_background: darwin_background(libc::PRIO_DARWIN_THREAD),
            thread_qos: thread_qos(),
            thread_priority,
            thread_base_priority,
            task_role: task_role(),
            load_average: load_average(),
        }
    }

    /// `getpriority(PRIO_DARWIN_*, 0)` answers 1 when darwin-background is
    /// set and 0 when not; -1 with `errno` set is a failure.
    fn darwin_background(which: libc::c_int) -> Option<bool> {
        // SAFETY: plain syscall on the calling process or thread.
        let value = unsafe {
            *libc::__error() = 0;
            libc::getpriority(which, 0)
        };
        // SAFETY: reads this thread's errno.
        if value == -1 && unsafe { *libc::__error() } != 0 {
            return None;
        }
        Some(value != 0)
    }

    fn thread_qos() -> Option<&'static str> {
        let mut class: u32 = 0;
        let mut relative: libc::c_int = 0;
        // SAFETY: out-pointers to locals; `pthread_self` is always valid.
        let rc =
            unsafe { pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative) };
        (rc == 0).then(|| qos_name(class))
    }

    fn thread_priorities() -> (Option<i32>, Option<i32>) {
        let mut info = std::mem::MaybeUninit::<libc::thread_extended_info>::zeroed();
        let mut count = libc::THREAD_EXTENDED_INFO_COUNT;
        // SAFETY: `pthread_mach_thread_np` returns this thread's port without
        // taking a reference; `info` is sized by `count`.
        let rc = unsafe {
            libc::thread_info(
                libc::pthread_mach_thread_np(libc::pthread_self()),
                libc::THREAD_EXTENDED_INFO as libc::thread_flavor_t,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if rc != libc::KERN_SUCCESS {
            return (None, None);
        }
        // SAFETY: the kernel filled it on success; it started zeroed anyway.
        let info = unsafe { info.assume_init() };
        (Some(info.pth_curpri), Some(info.pth_priority))
    }

    fn task_role() -> Option<&'static str> {
        let mut role: libc::integer_t = 0;
        let mut count: libc::mach_msg_type_number_t = 1;
        let mut get_default: libc::boolean_t = 0;
        // SAFETY: `task_category_policy` is a single `integer_t`;
        // `mach_task_self_` is set before `main` and never changes.
        let rc = unsafe {
            task_policy_get(
                mach_task_self_,
                TASK_CATEGORY_POLICY,
                &mut role,
                &mut count,
                &mut get_default,
            )
        };
        (rc == libc::KERN_SUCCESS).then(|| role_name(role))
    }

    fn load_average() -> Option<[f64; 3]> {
        let mut loads = [0f64; 3];
        // SAFETY: three doubles for three samples.
        let n = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
        (n == 3).then_some(loads)
    }

    pub(super) fn qos_name(class: u32) -> &'static str {
        match class {
            0x21 => "user-interactive",
            0x19 => "user-initiated",
            0x15 => "default",
            0x11 => "utility",
            0x09 => "background",
            0x00 => "unspecified",
            _ => "unknown",
        }
    }

    pub(super) fn role_name(role: libc::integer_t) -> &'static str {
        match role {
            -1 => "reniced",
            0 => "unspecified",
            1 => "foreground",
            2 => "background",
            3 => "control",
            4 => "graphics-server",
            5 => "throttle",
            6 => "nonui",
            7 => "default",
            8 => "darwinbg",
            _ => "unknown",
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::SchedState;

    pub(super) fn current() -> SchedState {
        SchedState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_throttled_snapshot_names_the_background_flags_first() {
        let state = SchedState {
            process_background: Some(true),
            thread_background: Some(false),
            thread_qos: Some("user-initiated"),
            thread_priority: Some(4),
            thread_base_priority: Some(31),
            task_role: Some("background"),
            load_average: Some([155.25, 52.4, 12.6]),
        };
        assert_eq!(
            state.to_string(),
            "pri 4/31, darwin-bg process, qos user-initiated, role background, load 155.2/52.4/12.6"
        );
    }

    #[test]
    fn an_empty_snapshot_reads_as_unavailable() {
        assert_eq!(SchedState::default().to_string(), "unavailable");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reports_this_thread() {
        let state = current();
        // Every query answers for an ordinary thread. Whether it is throttled
        // depends on how the test runner was launched, so that is not checked.
        assert!(state.process_background.is_some());
        assert!(state.thread_background.is_some());
        assert!(state.thread_priority.is_some_and(|p| p > 0));
        assert!(state.thread_qos.is_some());
        assert!(state.task_role.is_some());
        assert!(state.load_average.is_some());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_names_known_classes_and_roles() {
        assert_eq!(platform::qos_name(0x19), "user-initiated");
        assert_eq!(platform::qos_name(0x42), "unknown");
        assert_eq!(platform::role_name(2), "background");
        assert_eq!(platform::role_name(99), "unknown");
    }
}
