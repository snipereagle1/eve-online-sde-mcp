# Issue tracker: GitHub

Issues and PRDs for this repo live as GitHub issues. Use the `gh` CLI for all operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`. Use a heredoc for multi-line bodies.
- **Read an issue**: `gh issue view <number> --comments`, filtering comments by `jq` and also fetching labels.
- **List issues**: `gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters.
- **Comment on an issue**: `gh issue comment <number> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --add-label "..."` / `--remove-label "..."`
- **Close**: `gh issue close <number> --comment "..."`

Infer the repo from `git remote -v` — `gh` does this automatically when run inside a clone.

## A child ticket does not contain its own spec

A ticket with a `## Parent` link is a slice of that parent, not a self-contained
brief. The parent epic carries the binding Implementation Decisions, Testing
Decisions and Out of Scope sections; the child carries only the slice's scope and
acceptance criteria. **Read the parent before starting a child**, and when
dispatching a child to an agent, point it at the parent by number as well as at
any ADR.

This is not hypothetical bookkeeping. On epic #37 two child tickets were
implemented without their parent: one added the exact `scan.rs` unit tests the
epic's Testing Decisions prohibit, and missed a `maximum 1000` limit ceiling the
ADR does not mention. Both had to be undone. An ADR records *why* a design was
chosen; the epic records *what* to build and *how it will be tested*. They are
not substitutes.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.
