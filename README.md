# auto-yes

Answers "Yes" to Claude Code permission prompts, for only the sessions you attach.

It is a `PermissionRequest` hook, not a screen-scraping bot. Claude Code runs the hook
whenever it is about to show a "Do you want to proceed?" dialog. If that session is
attached, the hook answers allow. Attaching only creates a flag file in
`~/.claude/auto-yes/attached/`, so it takes effect on the very next prompt. Sessions that
are already running pick the hook up live, and nothing has to keep running in the
background.

Install: put `auto-yes` on your PATH (`ln -s "$PWD/auto-yes" ~/.local/bin/auto-yes`),
then run `auto-yes install`. It needs only Python 3, with no extra packages.

## Live view

Run `auto-yes` with no arguments to open a live view in your terminal. It refreshes every
second, shows every running Claude Code session, and shows approvals as they happen.

```
auto-yes live · answers Yes to permission prompts in attached sessions
hook installed · 1 of 3 attached

       PID    SESSION   NAME                          FOLDER
›  ON  13372  f6423692  bsf-85                        ~/Desktop/bsf
   off 84600  db1a9798  Markdown files context overv  ~/Desktop/deep-learning
   off 66487  1e7f99ab  auto-yes-claude-7c            ~/Desktop/auto-yes-claude

recent auto-approvals
  2026-11-25 09:16:17  f6423692  Bash: git push origin main

↑↓ move   space/enter toggle   a all on/off   i install   q quit
```

Closing the view doesn't detach anything, because attachment is stored in flag files.

## Commands

```
auto-yes install            # once: adds the hook to ~/.claude/settings.json (backup kept)
auto-yes list               # live sessions (* = attached)
auto-yes attach             # the session running in the current folder
auto-yes attach 66487       # ...or by pid, session-id prefix, name, or folder
auto-yes attach --all       # every session, including future ones
auto-yes detach [target|--all]
auto-yes log                # what was auto-approved, newest last
auto-yes uninstall
```

Notes
- It approves **everything** that prompts in an attached session, including the
  Gate D asks from `~/.claude/hooks/fable-gates.sh` (`git push`, `rm -rf`, `--force`).
  Deny rules still apply, because a denied call never reaches a prompt.
- `AskUserQuestion` is never answered, because "allow" there would skip your answer
  instead of giving one.
- It only works in the interactive TUI. In `claude -p` a hook's ask becomes a denial
  before any `PermissionRequest` hook runs.
- Sessions launched from inside another Claude session (child sessions) don't register in
  `~/.claude/sessions/`, so they can't be targeted; `attach --all` still covers them.
