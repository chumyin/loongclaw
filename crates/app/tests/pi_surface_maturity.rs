use loong_app::chat::pi_surface::{diff_viewer::render_diff_to_lines, message_list::MessageList};

fn line_texts(lines: impl IntoIterator<Item = ratatui::text::Line<'static>>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn diff_viewer_keeps_empty_and_contextual_diffs_legible() {
    let empty = line_texts(render_diff_to_lines(""));
    assert_eq!(empty, vec!["  (empty diff)".to_owned()]);

    let rendered = line_texts(render_diff_to_lines(
        " context line\n-old value\n+new value\n trailing context",
    ));

    assert_eq!(rendered[0], "   context line");
    assert!(rendered.iter().any(|line| line.contains("- old value")));
    assert!(rendered.iter().any(|line| line.contains("+ new value")));
    assert_eq!(
        rendered.last().map(String::as_str),
        Some("   trailing context")
    );
}

#[test]
fn message_list_renders_diff_code_and_tables_consistently_in_one_reply() {
    let mut list = MessageList::new();
    list.add_assistant_message(
        "### Patch\n```diff\n-old value\n+new value\n```\n\n### Commands\n```bash\nnpm install\nnpm test\n```\n\n| Metric | Value |\n| --- | --- |\n| coverage | 68% |\n| p95 | 220ms |"
            .to_owned(),
    );

    let rendered = line_texts(list.get_rendered_lines(52)).join("\n");

    assert!(rendered.contains("[Patch]"));
    assert!(rendered.contains("- old value"));
    assert!(rendered.contains("+ new value"));
    assert!(!rendered.contains("```diff"));
    assert!(rendered.contains("```bash"));
    assert!(rendered.contains("npm install"));
    assert!(rendered.contains("npm test"));
    assert!(rendered.contains("┌"));
    assert!(rendered.contains("coverage"));
    assert!(rendered.contains("220ms"));
    assert!(!rendered.contains("| --- |"));
}

#[test]
fn narrow_message_list_keeps_table_and_code_blocks_readable() {
    let mut list = MessageList::new();
    list.add_assistant_message(
        "### Commands\n```bash\ncargo test -p loong-app --lib\n```\n\n| Metric | Value |\n| --- | --- |\n| coverage | 68% |"
            .to_owned(),
    );

    let rendered = line_texts(list.get_rendered_lines(18)).join("\n");

    assert!(rendered.contains("```bash"));
    assert!(rendered.contains("cargo test"));
    assert!(rendered.contains("Metric: coverage"));
    assert!(rendered.contains("Value: 68%"));
}
