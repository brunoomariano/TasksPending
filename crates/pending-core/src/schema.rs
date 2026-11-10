//! What Settings asks for each kind of source: what it is, the keys of its
//! stacks and how each is filled in, and the variables of the env file its
//! credentials come from. Whether what was filled in makes sense is up to
//! the code that loads the config; a test there holds these keys to the
//! ones each kind's stacks accept.

use crate::config::SourceKind;

/// How a stack key is filled in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// A line of text: a query, a filter, a path.
    Text,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// Any of these words.
    Choices(&'static [&'static str]),
    /// Free words: names, labels, repositories.
    Words,
    /// Yes or no; left out when unset.
    Toggle,
}

/// A key of a kind's stacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    /// Always written: the stack does not load without it.
    pub required: bool,
    pub input: Input,
}

/// A variable of the env file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Var {
    /// Its name, then the older or other names the source also reads, in
    /// the order it reads them.
    pub names: &'static [&'static str],
    pub label: &'static str,
    pub help: &'static str,
    /// A token or a secret address: typed hidden, and never shown back.
    pub secret: bool,
}

/// A kind of source, as Settings offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    pub kind: SourceKind,
    /// As the config writes it.
    pub id: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    /// The board a new source of this kind starts on.
    pub board: &'static str,
    /// What a stack shows, when it shows one of several things: exactly
    /// one of these keys is set.
    pub modes: &'static [Field],
    /// The other keys of its stacks.
    pub fields: &'static [Field],
    /// Where its credentials come from.
    pub vars: &'static [Var],
}

const SEVERITY: Field = Field {
    key: "severity",
    label: "Severity",
    help: "Of every card in the stack; without it, the source decides.",
    required: false,
    input: Input::Choice(&["info", "warning", "critical"]),
};
const ASSIGNEE: Field = Field {
    key: "assignee",
    label: "Assignee",
    help: "me (the default), none, others or any.",
    required: false,
    input: Input::Choice(&["me", "none", "others", "any"]),
};
const PRIORITY: Field = Field {
    key: "priority",
    label: "Priority",
    help: "Only these priorities; none picked is any.",
    required: false,
    input: Input::Choices(&["urgent", "high", "medium", "low", "none"]),
};
const STATE: Field = Field {
    key: "state",
    label: "States",
    help: "State names, e.g. In Review, compared without case.",
    required: false,
    input: Input::Words,
};
const WHEN: &[&str] = &["now", "today", "tomorrow", "later"];

/// Every kind Settings can add, in the order it lists them.
pub const KINDS: &[Kind] = &[
    Kind {
        kind: SourceKind::Github,
        id: "github",
        label: "GitHub",
        about: "Pull requests and issues by search, notifications and security alerts.",
        board: "Work",
        modes: &[
            Field {
                key: "query",
                label: "Search",
                help: "As typed in github.com's search, e.g. is:open is:pr review-requested:@me.",
                required: false,
                input: Input::Text,
            },
            Field {
                key: "notifications",
                label: "Notifications",
                help: "unread, or all (read ones too).",
                required: false,
                input: Input::Choice(&["unread", "all"]),
            },
            Field {
                key: "alerts",
                label: "Security alerts",
                help: "Open alerts of these owner/repo repositories.",
                required: false,
                input: Input::Words,
            },
        ],
        fields: &[SEVERITY],
        vars: &[Var {
            names: &["GITHUB_TOKEN", "GH_TOKEN"],
            label: "Token",
            help: "Optional: without one, `gh auth token` is asked.",
            secret: true,
        }],
    },
    Kind {
        kind: SourceKind::Gitlab,
        id: "gitlab",
        label: "GitLab",
        about: "Your open merge requests and issues, and your to-do items.",
        board: "Work",
        modes: &[
            Field {
                key: "merge_requests",
                label: "Merge requests",
                help: "Yours to review, assigned to you, or yours.",
                required: false,
                input: Input::Choice(&["review_requested", "assigned", "authored"]),
            },
            Field {
                key: "issues",
                label: "Issues",
                help: "Assigned to you, or yours.",
                required: false,
                input: Input::Choice(&["assigned", "authored"]),
            },
            Field {
                key: "todos",
                label: "To-do items",
                help: "Your pending to-do items.",
                required: false,
                input: Input::Toggle,
            },
        ],
        fields: &[
            Field {
                key: "group",
                label: "Group",
                help: "Only this group and its subgroups, e.g. acme/platform.",
                required: false,
                input: Input::Text,
            },
            Field {
                key: "project",
                label: "Project",
                help: "Only this project, e.g. acme/api (not with a group).",
                required: false,
                input: Input::Text,
            },
            Field {
                key: "labels",
                label: "Labels",
                help: "Only items with every one of these labels.",
                required: false,
                input: Input::Words,
            },
            Field {
                key: "draft",
                label: "Drafts",
                help: "Merge requests: only drafts when on, only the others when off.",
                required: false,
                input: Input::Toggle,
            },
            SEVERITY,
        ],
        vars: &[
            Var {
                names: &["GITLAB_TOKEN"],
                label: "Token",
                help: "A personal access token with the read_api scope.",
                secret: true,
            },
            Var {
                names: &["GITLAB_BASE_URL"],
                label: "Address",
                help: "Only for a self-managed instance, e.g. https://gitlab.example.com.",
                secret: false,
            },
        ],
    },
    Kind {
        kind: SourceKind::Plane,
        id: "plane",
        label: "Plane",
        about: "Work items of a workspace, filtered here.",
        board: "Work",
        modes: &[],
        fields: &[
            ASSIGNEE,
            Field {
                key: "state_group",
                label: "State groups",
                help: "Only these groups; with neither groups nor states, the open ones.",
                required: false,
                input: Input::Choices(&[
                    "backlog",
                    "unstarted",
                    "started",
                    "completed",
                    "cancelled",
                ]),
            },
            STATE,
            Field {
                key: "project",
                label: "Projects",
                help: "Project identifiers, e.g. API.",
                required: false,
                input: Input::Words,
            },
            PRIORITY,
        ],
        vars: &[
            Var {
                names: &["PLANE_BASE_URL"],
                label: "Address",
                help: "The API's address, e.g. https://plane.example.com.",
                secret: false,
            },
            Var {
                names: &["PLANE_WORKSPACE_SLUG", "PLANE_WORKSPACE"],
                label: "Workspace",
                help: "The workspace's slug, as in its address.",
                secret: false,
            },
            Var {
                names: &["PLANE_API_KEY", "PLANE_TOKEN"],
                label: "API key",
                help: "A personal API key.",
                secret: true,
            },
            Var {
                names: &["PLANE_WEB_URL"],
                label: "Web address",
                help: "Optional: where card links open, when it is not the API's address.",
                secret: false,
            },
        ],
    },
    Kind {
        kind: SourceKind::Jira,
        id: "jira",
        label: "Jira",
        about: "Issues by JQL, Jira Cloud or Data Center.",
        board: "Work",
        modes: &[],
        fields: &[
            Field {
                key: "jql",
                label: "JQL",
                help: "As typed in Jira's issue search, e.g. assignee = currentUser().",
                required: true,
                input: Input::Text,
            },
            SEVERITY,
        ],
        vars: &[
            Var {
                names: &["JIRA_BASE_URL"],
                label: "Address",
                help: "e.g. https://yourcompany.atlassian.net.",
                secret: false,
            },
            Var {
                names: &["JIRA_EMAIL"],
                label: "E-mail",
                help: "Jira Cloud: your account's e-mail, with the API token.",
                secret: false,
            },
            Var {
                names: &["JIRA_API_TOKEN"],
                label: "API token",
                help: "Jira Cloud.",
                secret: true,
            },
            Var {
                names: &["JIRA_TOKEN"],
                label: "Personal access token",
                help: "Jira Data Center, alone: not with an e-mail.",
                secret: true,
            },
        ],
    },
    Kind {
        kind: SourceKind::Linear,
        id: "linear",
        label: "Linear",
        about: "Issues of the workspace, filtered by Linear.",
        board: "Work",
        modes: &[],
        fields: &[
            ASSIGNEE,
            Field {
                key: "state_type",
                label: "State types",
                help: "Only these types; none picked is the open ones.",
                required: false,
                input: Input::Choices(&[
                    "triage",
                    "backlog",
                    "unstarted",
                    "started",
                    "completed",
                    "canceled",
                ]),
            },
            STATE,
            Field {
                key: "team",
                label: "Teams",
                help: "Team keys, e.g. ENG.",
                required: false,
                input: Input::Words,
            },
            PRIORITY,
        ],
        vars: &[Var {
            names: &["LINEAR_API_KEY"],
            label: "API key",
            help: "A personal API key.",
            secret: true,
        }],
    },
    Kind {
        kind: SourceKind::Todoist,
        id: "todoist",
        label: "Todoist",
        about: "Tasks by Todoist filter.",
        board: "Personal",
        modes: &[],
        fields: &[Field {
            key: "filter",
            label: "Filter",
            help: "A Todoist filter, e.g. today | overdue.",
            required: true,
            input: Input::Text,
        }],
        vars: &[Var {
            names: &["TODOIST_API_TOKEN"],
            label: "API token",
            help: "Todoist Settings → Integrations → Developer.",
            secret: true,
        }],
    },
    Kind {
        kind: SourceKind::Google,
        id: "google",
        label: "Google Calendar",
        about: "Events, through GNOME Online Accounts: no token here.",
        board: "Personal",
        modes: &[],
        fields: &[
            Field {
                key: "when",
                label: "When",
                help: "Only events now, today, tomorrow or later; none picked is all.",
                required: false,
                input: Input::Choices(WHEN),
            },
            Field {
                key: "calendar",
                label: "Calendars",
                help: "Names as Google Calendar shows them; none is every visible one.",
                required: false,
                input: Input::Words,
            },
        ],
        vars: &[],
    },
    Kind {
        kind: SourceKind::Ical,
        id: "ical",
        label: "Calendar (iCal)",
        about: "Events of an iCal feed, such as Google Calendar's secret address.",
        board: "Personal",
        modes: &[],
        fields: &[Field {
            key: "when",
            label: "When",
            help: "Events now, today, tomorrow or later; pick at least one.",
            required: true,
            input: Input::Choices(WHEN),
        }],
        vars: &[Var {
            names: &["TASKS_PENDING_ICAL_URL"],
            label: "Feed address",
            help: "The iCal address; treat it as a secret.",
            secret: true,
        }],
    },
    Kind {
        kind: SourceKind::Sample,
        id: "sample",
        label: "Sample",
        about: "Fixed cards, to try TasksPending out.",
        board: "Inbox",
        modes: &[],
        fields: &[],
        vars: &[],
    },
];

/// How Settings offers `kind`.
pub fn kind(kind: SourceKind) -> &'static Kind {
    KINDS
        .iter()
        .find(|offered| offered.kind == kind)
        .expect("every kind is offered")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each kind is offered once, under the name the config writes.
    #[test]
    fn offers_every_kind_once() {
        for offered in KINDS {
            let written: SourceKind = toml::Value::String(offered.id.to_owned())
                .try_into()
                .expect("a kind");
            assert_eq!(written, offered.kind, "{}", offered.id);
            assert_eq!(kind(offered.kind), offered);
        }
        let mut ids: Vec<_> = KINDS.iter().map(|offered| offered.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 9);
        assert!(KINDS.iter().all(|offered| !offered.about.is_empty()));
    }
}
