use std::path::Path;

use citadel_config::{SshConfig, SshHost, includes};

const FIXTURE: &str = concat!(
    "# Global defaults\n",
    "Include ~/.ssh/config.d/*\n",
    "Include \"conf with space/extra\"\n",
    "ServerAliveInterval 30\n",
    "\tSendEnv LANG LC_*\n",
    "\n",
    "Host bastion\n",
    "    HostName bastion.example.com\n",
    "    User  ops\n",
    "    Port=2222\n",
    "    IdentityFile ~/.ssh/id_ed25519\n",
    "    IdentityFile \"~/.ssh/id with space\"\n",
    "\tForwardAgent yes   \n",
    "\n",
    "Host web-*  !web-legacy\n",
    "  ProxyJump bastion\n",
    "  User = deploy\n",
    "\n",
    "Host db1 db2\n",
    "    HostName %h.internal\n",
    "    ProxyJump bastion,jump2\n",
    "    LocalForward 5432 localhost:5432\n",
    "    # inline-ish comment\n",
    "    DynamicForward 1080\n",
    "\n",
    "Match host *.corp exec \"test -f /etc/corp\"\n",
    "    User corp\n",
    "\n",
    "Host *\n",
    "    IdentitiesOnly yes\n",
    "    User fallback\n",
    "    ServerAliveCountMax 3\n",
);

const FRAGMENTS: &[&str] = &[
    "Host a",
    "Host\tTabbed",
    "Host",
    "  HostName example.org",
    "HostName=example.org",
    "HostName = \"quoted value\"",
    "\tUser root   ",
    "Port",
    "=orphan",
    "Key\"quoted",
    "# comment",
    "#no space",
    "   # indented comment",
    "",
    "   ",
    "\t\t",
    "\r",
    "HostName héllo-ünïcode",
    "IdentityFile C:\\Users\\me\\.ssh\\id",
    "LocalForward 8080 [::1]:80",
    "Match all",
    "Match exec \"echo 'a b'\"",
    "Include *.conf",
    "ProxyJump a,b,c # trailing comment",
    "User 'single quoted'",
    "SendEnv \\\"",
];

const EOLS: &[&str] = &["\n", "\r\n", "", "\n\n"];

fn lcg(state: &mut u64) -> usize {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as usize
}

#[test]
fn fixture_roundtrips_byte_exact() {
    assert_eq!(SshConfig::parse(FIXTURE).render(), FIXTURE);
    let crlf = FIXTURE.replace('\n', "\r\n");
    assert_eq!(SshConfig::parse(&crlf).render(), crlf);
    let open = FIXTURE.trim_end_matches('\n');
    assert_eq!(SshConfig::parse(open).render(), open);
    assert_eq!(SshConfig::parse("").render(), "");
    assert_eq!(SshConfig::parse("\n").render(), "\n");
    assert_eq!(SshConfig::parse("\r\n\r\n").render(), "\r\n\r\n");
    assert_eq!(SshConfig::parse("Host x\r").render(), "Host x\r");
}

#[test]
fn generated_inputs_roundtrip_byte_exact() {
    let mut state = 0x5eed_u64;
    for _ in 0..2000 {
        let n = lcg(&mut state) % 24;
        let mut src = String::new();
        for _ in 0..n {
            src.push_str(FRAGMENTS[lcg(&mut state) % FRAGMENTS.len()]);
            src.push_str(EOLS[lcg(&mut state) % EOLS.len()]);
        }
        let cfg = SshConfig::parse(&src);
        assert_eq!(cfg.render(), src);
        let _ = cfg.hosts();
    }
}

#[test]
fn hosts_follow_openssh_first_value_wins() {
    let cfg = SshConfig::parse(FIXTURE);
    let hosts = cfg.hosts();
    let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
    assert_eq!(aliases, vec!["bastion", "db1", "db2"]);

    let b = &hosts[0];
    assert_eq!(b.hostname.as_deref(), Some("bastion.example.com"));
    assert_eq!(b.user.as_deref(), Some("ops"));
    assert_eq!(b.port, Some(2222));
    assert_eq!(
        b.identity_files,
        vec!["~/.ssh/id_ed25519", "~/.ssh/id with space"]
    );
    assert_eq!(b.forward_agent, Some(true));
    assert_eq!(b.identities_only, Some(true));
    assert!(b.proxy_jump.is_empty());

    let d = &hosts[1];
    assert_eq!(d.hostname.as_deref(), Some("%h.internal"));
    assert_eq!(d.user.as_deref(), Some("fallback"));
    assert_eq!(d.proxy_jump, vec!["bastion", "jump2"]);
    assert_eq!(d.local_forwards, vec!["5432 localhost:5432"]);
    assert_eq!(d.dynamic_forwards, vec!["1080"]);
    assert_eq!(hosts[2].proxy_jump, d.proxy_jump);
}

#[test]
fn earlier_wildcards_and_globals_take_precedence() {
    let cfg =
        SshConfig::parse("User global\nHost *\n  Port 2200\nHost x\n  Port 22\n  User late\n");
    let x = &cfg.hosts()[0];
    assert_eq!(x.port, Some(2200));
    assert_eq!(x.user.as_deref(), Some("global"));

    let cfg = SshConfig::parse("Host * !special\n  User a\nHost special other\n  User b\n");
    let hosts = cfg.hosts();
    assert_eq!(hosts[0].user.as_deref(), Some("b"));
    assert_eq!(hosts[1].user.as_deref(), Some("a"));

    let cfg = SshConfig::parse("Match all\n  Port 99\nMatch host z\n  Port 1\nHost z\n  Port 5\n");
    assert_eq!(cfg.hosts()[0].port, Some(99));

    let cfg = SshConfig::parse("Host j\n  ProxyJump none\nHost *\n  ProxyJump bastion\n");
    assert!(cfg.hosts()[0].proxy_jump.is_empty());

    let cfg = SshConfig::parse("HOST Up\n  HOSTNAME=up.example\n  forwardagent no\n");
    let up = &cfg.hosts()[0];
    assert_eq!(up.hostname.as_deref(), Some("up.example"));
    assert_eq!(up.forward_agent, Some(false));
}

#[test]
fn upsert_rewrites_only_the_changed_line() {
    let mut cfg = SshConfig::parse(FIXTURE);
    let mut b = cfg.hosts()[0].clone();
    b.identities_only = None;
    let mut cfg_local = SshConfig::parse(FIXTURE);
    let local = {
        let probe = SshConfig::parse(
            &FIXTURE[FIXTURE.find("Host bastion").unwrap()..FIXTURE.find("Host web-*").unwrap()],
        );
        probe.hosts()[0].clone()
    };
    cfg_local.upsert(&local);
    assert_eq!(cfg_local.render(), FIXTURE);

    b.port = Some(2200);
    cfg.upsert(&b);
    assert_eq!(
        cfg.render(),
        FIXTURE.replace("    Port=2222\n", "    Port=2200\n")
    );
}

#[test]
fn upsert_preserves_casing_separators_and_indentation() {
    let src = "# top\nHost app\n\tHOSTNAME = old.example   \n\tuser=root\n\n# next\nHost other\n\tUser x\n";
    let mut cfg = SshConfig::parse(src);
    let host = SshHost {
        alias: "app".into(),
        hostname: Some("new.example".into()),
        user: None,
        port: Some(2022),
        identity_files: vec!["~/.ssh/my key".into()],
        ..SshHost::default()
    };
    cfg.upsert(&host);
    let want = "# top\nHost app\n\tHOSTNAME = new.example   \n\tPort 2022\n\tIdentityFile \"~/.ssh/my key\"\n\n# next\nHost other\n\tUser x\n";
    assert_eq!(cfg.render(), want);
    assert_eq!(cfg.hosts()[0], host);
}

#[test]
fn upsert_multi_values_in_place() {
    let src = "Host m\n  IdentityFile a\n  User u\n  IdentityFile b\n  IdentityFile c\n  LocalForward 1 h:1\n";
    let mut cfg = SshConfig::parse(src);
    let mut h = cfg.hosts()[0].clone();
    h.identity_files = vec!["a".into(), "B".into()];
    h.local_forwards = vec!["1 h:1".into(), "2 \"h:2\"".into()];
    h.local_forwards[1] = "2 h:2".into();
    cfg.upsert(&h);
    assert_eq!(
        cfg.render(),
        "Host m\n  IdentityFile a\n  User u\n  IdentityFile B\n  LocalForward 1 h:1\n  LocalForward 2 h:2\n"
    );
    assert_eq!(cfg.hosts()[0], h);
}

#[test]
fn new_blocks_land_before_the_first_matching_catch_all() {
    let mut cfg = SshConfig::parse(FIXTURE);
    let fresh = SshHost {
        alias: "staging".into(),
        hostname: Some("10.0.0.9".into()),
        user: Some("ci".into()),
        proxy_jump: vec!["bastion".into(), "jump2".into()],
        forward_agent: Some(false),
        ..SshHost::default()
    };
    cfg.upsert(&fresh);
    let out = cfg.render();
    let block = "Host staging\n    HostName 10.0.0.9\n    User ci\n    ProxyJump bastion,jump2\n    ForwardAgent no\n\n";
    let at = out.find(block).expect("block present");
    assert!(at < out.find("Match host").unwrap());
    assert_eq!(out.replacen(block, "", 1), FIXTURE);
    let got = cfg
        .hosts()
        .into_iter()
        .find(|h| h.alias == "staging")
        .unwrap();
    assert_eq!(got.user.as_deref(), Some("ci"));
    assert_eq!(got.identities_only, Some(true));
}

#[test]
fn multi_alias_hosts_get_a_dedicated_block_that_wins() {
    let mut cfg = SshConfig::parse(FIXTURE);
    let mut db1 = cfg.hosts().into_iter().find(|h| h.alias == "db1").unwrap();
    db1.hostname = Some("db1.replica".into());
    cfg.upsert(&db1);
    let hosts = cfg.hosts();
    let got1 = hosts.iter().find(|h| h.alias == "db1").unwrap();
    let got2 = hosts.iter().find(|h| h.alias == "db2").unwrap();
    assert_eq!(got1.hostname.as_deref(), Some("db1.replica"));
    assert_eq!(got2.hostname.as_deref(), Some("%h.internal"));
    assert!(cfg.render().contains("Host db1 db2\n"));
}

#[test]
fn append_handles_crlf_and_missing_trailing_newline() {
    let mut cfg = SshConfig::parse("Host a\r\n  User x");
    cfg.upsert(&SshHost {
        alias: "b".into(),
        port: Some(2),
        ..SshHost::default()
    });
    assert_eq!(
        cfg.render(),
        "Host a\r\n  User x\r\n\r\nHost b\r\n    Port 2\r\n"
    );

    let mut empty = SshConfig::parse("");
    empty.upsert(&SshHost {
        alias: "solo".into(),
        ..SshHost::default()
    });
    assert_eq!(empty.render(), "Host solo\n");

    let mut bare = SshConfig::parse("Host a\n");
    bare.upsert(&SshHost {
        alias: "a".into(),
        user: Some("me".into()),
        ..SshHost::default()
    });
    assert_eq!(bare.render(), "Host a\n    User me\n");
}

#[test]
fn remove_drops_block_and_keeps_neighbours() {
    let src = "# top\nHost a\n  User x\n  # note\n  Port 1\n\n# about b\nHost b\n  User y\n\nHost c\n  User z\n";
    let mut cfg = SshConfig::parse(src);
    assert!(cfg.remove("b"));
    assert_eq!(
        cfg.render(),
        "# top\nHost a\n  User x\n  # note\n  Port 1\n\nHost c\n  User z\n"
    );
    assert!(cfg.remove("c"));
    assert_eq!(
        cfg.render(),
        "# top\nHost a\n  User x\n  # note\n  Port 1\n"
    );
    assert!(cfg.remove("a"));
    assert_eq!(cfg.render(), "# top\n");
    let mut tight = SshConfig::parse("Host a\n  User x\n# about b\nHost b\n  User y\n");
    assert!(tight.remove("b"));
    assert_eq!(tight.render(), "Host a\n  User x\n# about b\n");
    assert!(!cfg.remove("missing"));
    let mut multi = SshConfig::parse("Host d1 d2\n  User q\n");
    assert!(!multi.remove("d1"));
    assert_eq!(multi.render(), "Host d1 d2\n  User q\n");
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map_or(0, |m| m.permissions().mode() & 0o777)
}

#[test]
fn save_is_atomic_private_and_backs_up() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("home").join(".ssh");
    let path = ssh.join("config");
    assert_eq!(SshConfig::load(&path).unwrap().render(), "");

    let mut cfg = SshConfig::parse("Host one\n  User a\n");
    cfg.save(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "Host one\n  User a\n"
    );
    assert!(!ssh.join("config.citadel.bak").exists());
    #[cfg(unix)]
    {
        assert_eq!(mode(&ssh), 0o700);
        assert_eq!(mode(&path), 0o600);
    }

    cfg.upsert(&SshHost {
        alias: "two".into(),
        ..SshHost::default()
    });
    cfg.save(&path).unwrap();
    let bak = ssh.join("config.citadel.bak");
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        "Host one\n  User a\n"
    );
    assert_eq!(SshConfig::load(&path).unwrap(), cfg);
    #[cfg(unix)]
    assert_eq!(mode(&bak), 0o600);

    cfg.save(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        "Host one\n  User a\n"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&ssh)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".citadel-config-"))
        .collect();
    assert!(leftovers.is_empty());
}

#[cfg(unix)]
#[test]
fn save_follows_symlinked_config() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles-ssh-config");
    std::fs::write(&real, "Host old\n").unwrap();
    let link = dir.path().join("config");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut cfg = SshConfig::load(&link).unwrap();
    cfg.upsert(&SshHost {
        alias: "new".into(),
        ..SshHost::default()
    });
    cfg.save(&link).unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(&real).unwrap(),
        "Host old\n\nHost new\n"
    );
}

#[test]
fn load_rejects_non_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config");
    std::fs::write(&path, [0x48, 0x6f, 0xff, 0x0a]).unwrap();
    assert!(matches!(
        SshConfig::load(&path),
        Err(citadel_config::ConfigError::Encoding(_))
    ));
}

#[test]
fn include_paths_resolve_globs_relative_to_base() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::create_dir(base.join("config.d")).unwrap();
    for n in ["b.conf", "a.conf", "skip.txt"] {
        std::fs::write(base.join("config.d").join(n), "").unwrap();
    }
    std::fs::write(base.join("single"), "").unwrap();
    let abs = base.join("abs.conf");
    std::fs::write(&abs, "").unwrap();
    let src = format!(
        "Include config.d/*.conf single missing\nHost x\n  Include \"{}\"\n",
        abs.display()
    );
    let got = includes(&SshConfig::parse(&src), base);
    assert_eq!(
        got,
        vec![
            base.join("config.d").join("a.conf"),
            base.join("config.d").join("b.conf"),
            base.join("single"),
            abs,
        ]
    );
}

#[test]
fn values_with_control_characters_cannot_inject_directives() {
    let mut cfg = SshConfig::parse("");
    let host = SshHost {
        alias: "box".into(),
        hostname: Some("evil.example\nProxyCommand sh -c id".into()),
        ..SshHost::default()
    };
    cfg.upsert(&host);
    let text = cfg.render();
    assert!(
        !text.lines().any(|l| l
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("proxycommand")),
        "{text}"
    );
    assert_eq!(SshConfig::parse(&text).hosts().len(), 1);
}
