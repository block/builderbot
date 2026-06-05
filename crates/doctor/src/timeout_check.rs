use std::time::Duration;

use crate::command::CommandTimeout;
use crate::types::{AgentVersionInfo, AuthStatus, CheckStatus, DoctorCheck, InstallSource};

pub(crate) struct TimeoutCheck<'a> {
    id: String,
    label: String,
    status: CheckStatus,
    header: &'a str,
    command: String,
    timeout: Duration,
    path: Option<String>,
    bridge_path: Option<String>,
    install_source: Option<InstallSource>,
    auth_status: Option<AuthStatus>,
    main: Option<AgentVersionInfo>,
    bridge: Option<AgentVersionInfo>,
    raw_suffix: Option<&'a str>,
}

impl<'a> TimeoutCheck<'a> {
    pub(crate) fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        status: CheckStatus,
        header: &'a str,
        command: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status,
            header,
            command: command.into(),
            timeout,
            path: None,
            bridge_path: None,
            install_source: None,
            auth_status: None,
            main: None,
            bridge: None,
            raw_suffix: None,
        }
    }

    pub(crate) fn path(mut self, path: Option<String>) -> Self {
        self.path = path;
        self
    }

    pub(crate) fn install_source(mut self, install_source: Option<InstallSource>) -> Self {
        self.install_source = install_source;
        self
    }

    pub(crate) fn main(mut self, main: Option<AgentVersionInfo>) -> Self {
        self.main = main;
        self
    }

    pub(crate) fn raw_suffix(mut self, raw_suffix: Option<&'a str>) -> Self {
        self.raw_suffix = raw_suffix;
        self
    }
}

pub(crate) fn command_timeout_check(input: TimeoutCheck<'_>) -> DoctorCheck {
    let timeout = CommandTimeout::new(input.label.clone(), input.command, input.timeout);
    let mut raw = format!("{}\n{}", input.header, timeout.raw_output());
    if let Some(suffix) = input.raw_suffix {
        raw.push('\n');
        raw.push_str(suffix);
    }

    DoctorCheck {
        id: input.id,
        label: input.label,
        status: input.status,
        message: timeout.message(),
        fix_url: None,
        fix_command: None,
        fix_type: None,
        path: input.path,
        bridge_path: input.bridge_path,
        raw_output: Some(raw),
        auth_status: input.auth_status,
        installed_version: None,
        latest_version: None,
        update_available: None,
        install_source: input.install_source,
        self_updating: None,
        main: input.main,
        bridge: input.bridge,
    }
}
