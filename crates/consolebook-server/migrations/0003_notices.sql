-- Milestone 1: persisted in-app notices.
--
-- Workflow and operational events are shown in the application and never
-- depend on email delivery (docs/architecture.md Notifications). A notice
-- belongs to one recipient; reading it is recipient-scoped.

CREATE TABLE notice (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES user (id),
    kind TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    read_at INTEGER
) STRICT;

CREATE INDEX notice_user_unread ON notice (user_id, read_at);
