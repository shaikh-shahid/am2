use am2::terminals::{
    bounded_visible, matching_tty, parse_native_candidates, parse_target, Candidate, Provider,
};

#[test]
fn parses_terminal_and_iterm_sessions_with_exact_ttys() {
    let raw = "terminal\u{1f}41\u{1f}/dev/ttys001\u{1f}/dev/ttys001\u{1f}Shell\u{1f}10\u{1f}Terminal\u{1e}iterm2\u{1f}42\u{1f}ABC-123\u{1f}/dev/ttys002\u{1f}Agent\u{1f}11\u{1f}iTerm\u{1e}";
    let candidates = parse_native_candidates(raw).unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].provider, Provider::Terminal);
    assert_eq!(candidates[1].provider, Provider::ITerm2);
    assert_eq!(matching_tty(&candidates, "ttys002").len(), 1);
    assert_eq!(matching_tty(&candidates, "/dev/ttys002")[0].id, "ABC-123");
    assert!(matching_tty(&candidates, "??").is_empty());
    assert!(matching_tty(&candidates, "ttys999").is_empty());

    let mut duplicate = candidates[1].clone();
    duplicate.instance_pid = 99;
    let mut ambiguous = candidates;
    ambiguous.push(duplicate);
    assert_eq!(matching_tty(&ambiguous, "ttys002").len(), 2);
}

#[test]
fn target_keeps_provider_app_pid_and_session_identity() {
    let candidate = Candidate {
        provider: Provider::ITerm2,
        instance_pid: 42,
        id: "ABC-123".into(),
        tty: Some("/dev/ttys002".into()),
        title: "Agent".into(),
        cwd: None,
        window_id: "11".into(),
        window_title: "iTerm".into(),
    };
    assert_eq!(candidate.target(), "iterm2:42:ABC-123");
    assert_eq!(
        parse_target(&candidate.target()).unwrap().provider,
        Provider::ITerm2
    );
    assert!(parse_target("42:ABC-123").is_err());
}

#[test]
fn visible_terminal_text_keeps_only_a_bounded_tail() {
    let lines = (0..120)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = bounded_visible(&lines);
    assert_eq!(output.lines().count(), 80);
    assert!(output.contains("line 119"));
    assert!(!output.contains("line 0\n"));

    let oversized = format!("{}END", "x".repeat(20_000));
    let output = bounded_visible(&oversized);
    assert!(output.len() <= 16 * 1024);
    assert!(output.ends_with("END"));
}
