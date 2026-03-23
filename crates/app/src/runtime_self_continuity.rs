const SESSION_PROFILE_INTRO: &str = concat!(
    "Durable preferences and advisory session context carried into this session. ",
    "Future durable recall may enrich this section. ",
    "It does not override Resolved Runtime Identity.",
);

const MEMORY_SUMMARY_INTRO: &str = concat!(
    "Earlier session context condensed from turns outside the active window. ",
    "Treat it as session-local recall. ",
    "It does not replace Runtime Self Context. ",
    "It does not override Resolved Runtime Identity or Session Profile.",
);

const DURABLE_RECALL_INTRO: &str = concat!(
    "Advisory durable recall exported immediately before context compaction. ",
    "It may enrich future recall. ",
    "It does not replace Runtime Self Context. ",
    "It does not override Resolved Runtime Identity or Session Profile.",
);

const DELEGATE_CHILD_CONTINUITY_LINES: &[&str] = &[
    "- Runtime Self Context continues to supply standing instructions and soul guidance.",
    "- Resolved Runtime Identity remains the identity authority for this session chain.",
    concat!(
        "- Session Profile may carry durable advisory context, and future durable ",
        "recall can enrich it without overriding Resolved Runtime Identity."
    ),
    concat!(
        "- Memory Summary and child-task findings stay session-local unless a ",
        "separate durable-memory path promotes them."
    ),
];

pub(crate) const fn session_profile_intro() -> &'static str {
    SESSION_PROFILE_INTRO
}

pub(crate) const fn memory_summary_intro() -> &'static str {
    MEMORY_SUMMARY_INTRO
}

pub(crate) const fn durable_recall_intro() -> &'static str {
    DURABLE_RECALL_INTRO
}

pub(crate) const fn delegate_child_continuity_lines() -> &'static [&'static str] {
    DELEGATE_CHILD_CONTINUITY_LINES
}
