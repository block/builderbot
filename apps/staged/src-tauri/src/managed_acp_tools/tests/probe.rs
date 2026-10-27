use super::*;
use crate::managed_acp_tools::test_support::*;
use crate::managed_acp_tools::*;

/// The registry probe asks the snapshot's own `npm`, under the snapshot's
/// env, and reports only a URL-shaped answer.
#[tokio::test]
async fn configured_npm_registry_asks_the_snapshots_npm() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_executable(
        &bin.join("npm"),
        "#!/bin/sh\n[ \"$1 $2 $3\" = \"config get registry\" ] || exit 2\nprintf '%s\\n' \"${FAKE_REGISTRY:-https://mirror.example.test/npm/}\"\n",
    );
    let env = vec![("PATH".to_string(), bin.display().to_string())];
    assert_eq!(
        configured_npm_registry(&env).await.as_deref(),
        Some("https://mirror.example.test/npm/")
    );

    // The env is the snapshot's, not the process's.
    let mut env_with_registry = env.clone();
    env_with_registry.push((
        "FAKE_REGISTRY".to_string(),
        "https://other.example.test/".to_string(),
    ));
    assert_eq!(
        configured_npm_registry(&env_with_registry).await.as_deref(),
        Some("https://other.example.test/")
    );

    // Not a URL: no answer. No npm on the snapshot PATH: no answer.
    let mut env_garbage = env.clone();
    env_garbage.push(("FAKE_REGISTRY".to_string(), "undefined".to_string()));
    assert_eq!(configured_npm_registry(&env_garbage).await, None);
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        configured_npm_registry(&[("PATH".to_string(), empty.display().to_string())]).await,
        None
    );
}

/// Asking twice under one snapshot spawns npm once: the answer — a URL or
/// `None` alike — is reused within the TTL, so a Doctor refresh never
/// re-pays the probe. A different snapshot is a different question.
#[tokio::test]
async fn configured_npm_registry_is_cached_per_env_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let calls = dir.path().join("calls");
    write_executable(
        &bin.join("npm"),
        &format!(
            "#!/bin/sh\nprintf x >> '{}'\nprintf '%s\\n' \"${{FAKE_REGISTRY:-https://mirror.example.test/npm/}}\"\n",
            calls.display()
        ),
    );
    let spawns = || {
        std::fs::read_to_string(&calls)
            .map(|s| s.len())
            .unwrap_or(0)
    };
    let env = vec![("PATH".to_string(), bin.display().to_string())];

    assert_eq!(
        configured_npm_registry(&env).await.as_deref(),
        Some("https://mirror.example.test/npm/")
    );
    assert_eq!(
        configured_npm_registry(&env).await.as_deref(),
        Some("https://mirror.example.test/npm/")
    );
    assert_eq!(spawns(), 1, "the second ask is answered from the cache");

    // A `None` answer is cached as well.
    let mut env_garbage = env.clone();
    env_garbage.push(("FAKE_REGISTRY".to_string(), "undefined".to_string()));
    assert_eq!(configured_npm_registry(&env_garbage).await, None);
    assert_eq!(configured_npm_registry(&env_garbage).await, None);
    assert_eq!(spawns(), 2);
}

#[test]
fn npm_registry_cache_expires_after_the_ttl() {
    let mut cache = NpmRegistryCache::default();
    let now = Instant::now();
    let env = vec![("PATH".to_string(), "/x".to_string())];
    let other = vec![("PATH".to_string(), "/y".to_string())];
    assert_eq!(cache.get_fresh(&env, now), None);

    cache.insert(&env, Some("https://r.example.test/".to_string()), now);
    cache.insert(&other, None, now);
    assert_eq!(
        cache.get_fresh(&env, now + Duration::from_secs(30)),
        Some(Some("https://r.example.test/".to_string()))
    );
    assert_eq!(
        cache.get_fresh(&other, now),
        Some(None),
        "a cached `None` is an answer, not a miss"
    );
    assert_eq!(
        cache.get_fresh(&env, now + NPM_REGISTRY_CACHE_TTL + Duration::from_secs(1)),
        None
    );
}

/// A fake `npm` on a PATH dir: prints `version` for `npm view <pkg>
/// version`, exits `exit_code`, and refuses unless `--registry` is passed
/// exactly when `expect_registry` says so.
fn write_fake_npm(bin_dir: &Path, version: &str, exit_code: i32, expect_registry: bool) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let registry_check = if expect_registry {
        "case \" $* \" in *\" --registry \"*) ;; *) echo 'missing --registry' >&2; exit 43;; esac\n"
    } else {
        "case \" $* \" in *\" --registry \"*) echo 'unexpected --registry' >&2; exit 43;; esac\n"
    };
    write_executable(
        &bin_dir.join("npm"),
        &format!(
            "#!/bin/sh\ntest \"$1\" = view || exit 44\ntest \"$3\" = version || exit 45\n{registry_check}echo '{version}'\nexit {exit_code}\n"
        ),
    );
}

fn path_env(bin_dir: &Path) -> Vec<(String, String)> {
    vec![(
        "PATH".to_string(),
        format!("{}:/usr/bin:/bin", bin_dir.display()),
    )]
}

#[tokio::test]
async fn npm_view_version_reads_the_registry_answer() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    write_fake_npm(&bin, "4.5.6", 0, true);
    let env = path_env(&bin);
    let npm = find_on_env_path(&env, "npm").expect("npm resolves from the snapshot PATH");
    assert_eq!(npm, bin.join("npm"));

    let version = npm_view_version(
        &npm,
        "@agentclientprotocol/claude-agent-acp",
        Some("https://registry.example.test/npm/"),
        &env,
        Duration::from_secs(10),
    )
    .await
    .ok();
    assert_eq!(version.as_deref(), Some("4.5.6"));
}

#[tokio::test]
async fn npm_view_version_omits_the_registry_flag_when_none_is_configured() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    write_fake_npm(&bin, "4.5.6", 0, false);
    let env = path_env(&bin);

    let version = npm_view_version(
        &bin.join("npm"),
        "@agentclientprotocol/codex-acp",
        None,
        &env,
        Duration::from_secs(10),
    )
    .await
    .ok();
    assert_eq!(version.as_deref(), Some("4.5.6"));
}

/// Probes must not inherit npm's ~70 s dead-host retry schedule: retries
/// are off and npm's own fetch timeout is pinned to the probe budget.
#[tokio::test]
async fn npm_view_version_bounds_npm_fetch_retries_and_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let argv_log = dir.path().join("argv.log");
    write_executable(
        &bin.join("npm"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\necho 1.0.0\n",
            argv_log.display()
        ),
    );
    let env = path_env(&bin);
    npm_view_version(
        &bin.join("npm"),
        "@agentclientprotocol/codex-acp",
        None,
        &env,
        Duration::from_secs(15),
    )
    .await
    .unwrap();
    let argv = std::fs::read_to_string(&argv_log).unwrap();
    assert!(argv.contains(" --fetch-retries=0"), "{argv}");
    assert!(argv.contains(" --fetch-timeout=15000"), "{argv}");
}

/// A fake `npm view` that reports a gateway block page unless `--registry`
/// is passed, logging each invocation's argv.
fn write_fake_gateway_blocked_npm_view(bin_dir: &Path, argv_log: &Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    write_executable(
        &bin_dir.join("npm"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \" $* \" in *\" --registry \"*) echo 7.8.9; exit 0;; esac\necho 'npm error code FETCH_ERROR' >&2\necho 'npm error request to https://registry.npmjs.org/x failed: https://blocked.teams.cloudflare.com?block_reason=x' >&2\nexit 1\n",
            argv_log.display()
        ),
    );
}

/// The freshness probe follows the install's precedence: npm's own config
/// first, then — for a gateway block with no override — one retry through
/// the fallback.
#[tokio::test]
async fn probe_latest_version_retries_a_gateway_block_through_the_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let argv_log = dir.path().join("argv.log");
    write_fake_gateway_blocked_npm_view(&bin, &argv_log);
    let env = path_env(&bin);
    let package = "@agentclientprotocol/claude-agent-acp";
    let fallback = Some("https://fallback.example.test/npm/");

    let version = probe_latest_version(
        &bin.join("npm"),
        package,
        None,
        fallback,
        &env,
        Duration::from_secs(15),
    )
    .await;
    assert_eq!(version.as_deref(), Some("7.8.9"));
    let argv: Vec<String> = std::fs::read_to_string(&argv_log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(argv.len(), 2, "{argv:?}");
    assert!(!argv[0].contains("--registry"), "{}", argv[0]);
    assert!(
        argv[1].contains("--registry https://fallback.example.test/npm/"),
        "{}",
        argv[1]
    );
    assert!(
        argv[1].contains("--@agentclientprotocol:registry=https://fallback.example.test/npm/"),
        "{}",
        argv[1]
    );

    // No fallback compiled in: the block page is simply unknown.
    std::fs::remove_file(&argv_log).unwrap();
    assert_eq!(
        probe_latest_version(
            &bin.join("npm"),
            package,
            None,
            None,
            &env,
            Duration::from_secs(15)
        )
        .await,
        None
    );
    assert_eq!(
        std::fs::read_to_string(&argv_log).unwrap().lines().count(),
        1
    );

    // An override is used as-is and never falls back.
    std::fs::remove_file(&argv_log).unwrap();
    assert_eq!(
        probe_latest_version(
            &bin.join("npm"),
            package,
            Some("https://mine.example.test/npm/"),
            fallback,
            &env,
            Duration::from_secs(15)
        )
        .await
        .as_deref(),
        Some("7.8.9")
    );
    let argv = std::fs::read_to_string(&argv_log).unwrap();
    assert_eq!(argv.lines().count(), 1, "{argv}");
    assert!(argv.contains("--registry https://mine.example.test/npm/"));
    assert!(argv.contains("--@agentclientprotocol:registry=https://mine.example.test/npm/"));
}

/// Every failure mode reads as unknown — never as a version.
#[tokio::test]
async fn npm_view_version_is_unknown_on_failure_garbage_or_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let env = path_env(&bin);
    let package = "@agentclientprotocol/claude-agent-acp";

    // A registry error: npm exits non-zero.
    write_fake_npm(&bin, "4.5.6", 1, false);
    assert_eq!(
        npm_view_version(
            &bin.join("npm"),
            package,
            None,
            &env,
            Duration::from_secs(10)
        )
        .await
        .ok(),
        None
    );

    // A clean exit with non-version output (an npm notice, an HTML page).
    write_fake_npm(&bin, "npm notice: try again later", 0, false);
    assert_eq!(
        npm_view_version(
            &bin.join("npm"),
            package,
            None,
            &env,
            Duration::from_secs(10)
        )
        .await
        .ok(),
        None
    );

    // A hung lookup is cut off at the bound.
    write_executable(&bin.join("npm"), "#!/bin/sh\nsleep 30\necho 9.9.9\n");
    let started = Instant::now();
    assert_eq!(
        npm_view_version(
            &bin.join("npm"),
            package,
            None,
            &env,
            Duration::from_millis(200)
        )
        .await
        .ok(),
        None
    );
    assert!(started.elapsed() < Duration::from_secs(10));

    // No npm anywhere on the snapshot PATH: nothing to ask.
    let empty = vec![("PATH".to_string(), "/nonexistent/dir".to_string())];
    assert_eq!(find_on_env_path(&empty, "npm"), None);
    assert_eq!(find_on_env_path(&[], "npm"), None);
}

#[test]
fn latest_version_cache_expires_after_the_ttl() {
    let env = vec![("HOME".to_string(), "/nonexistent".to_string())];
    let key = LatestVersionKey::new("pkg", &env);
    let other = LatestVersionKey::new("other", &env);
    let mut cache = LatestVersionCache::default();
    let now = Instant::now();
    assert_eq!(cache.get_fresh(&key, now), None);

    cache.insert(key.clone(), "1.2.3".to_string(), now);
    assert_eq!(
        cache
            .get_fresh(&key, now + Duration::from_secs(60))
            .as_deref(),
        Some("1.2.3")
    );
    assert_eq!(cache.get_fresh(&other, now), None);
    let expired = now + LATEST_VERSION_CACHE_TTL + Duration::from_secs(1);
    assert_eq!(cache.get_fresh(&key, expired), None);

    // An insert past the TTL also drops the expired entry.
    cache.insert(other.clone(), "2.0.0".to_string(), expired);
    assert!(!cache.entries.contains_key(&key));
    assert_eq!(cache.get_fresh(&other, expired).as_deref(), Some("2.0.0"));
}

/// The cache is keyed by what decides the registry, not just the package: a
/// different override, env snapshot or user npmrc is a different entry.
#[test]
fn latest_version_cache_is_keyed_by_registry_configuration() {
    let env = vec![("HOME".to_string(), "/nonexistent".to_string())];
    let key = LatestVersionKey::new("pkg", &env);
    let mut cache = LatestVersionCache::default();
    let now = Instant::now();
    cache.insert(key.clone(), "1.2.3".to_string(), now);
    assert_eq!(cache.get_fresh(&key, now).as_deref(), Some("1.2.3"));

    let mut other_override = key.clone();
    other_override.registry.override_registry = Some("https://mine.example.test/npm/".to_string());
    assert_eq!(cache.get_fresh(&other_override, now), None);

    let mut env_with_registry = env.clone();
    env_with_registry.push((
        "NPM_CONFIG_REGISTRY".to_string(),
        "https://mirror.example.test/npm/".to_string(),
    ));
    let other_env = LatestVersionKey::new("pkg", &env_with_registry);
    assert_eq!(cache.get_fresh(&other_env, now), None);

    let mut other_npmrc = key.clone();
    other_npmrc.registry.user_npmrc = Some((PathBuf::from("/nonexistent/.npmrc"), 12, None));
    assert_eq!(cache.get_fresh(&other_npmrc, now), None);
}

/// The user npmrc is the one npm reads under the snapshot: the userconfig
/// override in either spelling, else `$HOME/.npmrc`.
#[test]
fn user_npmrc_path_follows_the_snapshots_npm_config() {
    let home = vec![("HOME".to_string(), "/home/u".to_string())];
    assert_eq!(
        user_npmrc_path(&home),
        Some(PathBuf::from("/home/u/.npmrc"))
    );
    let mut lower = home.clone();
    lower.push((
        "npm_config_userconfig".to_string(),
        "/etc/alt-npmrc".to_string(),
    ));
    assert_eq!(
        user_npmrc_path(&lower),
        Some(PathBuf::from("/etc/alt-npmrc"))
    );
    let mut upper = lower.clone();
    upper.push((
        "NPM_CONFIG_USERCONFIG".to_string(),
        "/etc/upper-npmrc".to_string(),
    ));
    assert_eq!(
        user_npmrc_path(&upper),
        Some(PathBuf::from("/etc/upper-npmrc"))
    );
    assert_eq!(user_npmrc_path(&[]), None);
}

/// Editing the user npmrc (here the one `NPM_CONFIG_USERCONFIG` names) makes
/// the next ask probe again within the TTL; an untouched one is served from
/// the cache even once npm is gone.
#[tokio::test]
async fn latest_published_version_reprobes_when_the_user_npmrc_changes() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let npmrc = dir.path().join("npmrc");
    std::fs::create_dir_all(&bin).unwrap();
    // The fake answers with the npmrc's contents, so the answer itself tells
    // whether npm was asked again.
    write_executable(
        &bin.join("npm"),
        "#!/bin/sh\ntest \"$1\" = view || exit 44\ncat \"$NPM_CONFIG_USERCONFIG\"\n",
    );
    let tool = ManagedTool {
        id: "cache-key-probe",
        binary: "cache-key-probe",
        package: "@staged-test/cache-key-probe",
    };
    let mut env = path_env(&bin);
    env.push((
        "NPM_CONFIG_USERCONFIG".to_string(),
        npmrc.display().to_string(),
    ));

    std::fs::write(&npmrc, "1.0.0\n").unwrap();
    assert_eq!(
        latest_published_version(&tool, &env).await.as_deref(),
        Some("1.0.0")
    );
    // A different length, so the fingerprint changes even within one mtime tick.
    std::fs::write(&npmrc, "2.0.0-next\n").unwrap();
    assert_eq!(
        latest_published_version(&tool, &env).await.as_deref(),
        Some("2.0.0-next")
    );
    std::fs::remove_file(bin.join("npm")).unwrap();
    assert_eq!(
        latest_published_version(&tool, &env).await.as_deref(),
        Some("2.0.0-next"),
        "an unchanged npmrc is a cache hit"
    );
}
