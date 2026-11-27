<p align="center">
  <img src="assets/banner.png" alt="Grandaddy: manage your vibe coding sessions, all in one place" width="820">
</p>

<p align="center">
  <b>Multi-session management</b>&nbsp;&nbsp;·&nbsp;&nbsp;<b>Keep your context</b>&nbsp;&nbsp;·&nbsp;&nbsp;<b>Ship more</b>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-2b2b2b?style=flat-square"></a>
  <img alt="Written in Rust" src="https://img.shields.io/badge/written%20in-Rust-2b2b2b?style=flat-square">
  <img alt="macOS app built with Tauri" src="https://img.shields.io/badge/app-Tauri-2b2b2b?style=flat-square">
  <img alt="Tested on macOS" src="https://img.shields.io/badge/tested%20on-macOS-2b2b2b?style=flat-square">
</p>

**grandaddy** runs every Claude Code session on your Mac from one place: in your terminal, in the
background, or in the Claude app. See what each one is doing, answer what it's waiting on, and
drive it: send prompts and slash commands, switch its mode, change its model or effort, interrupt
it. You can also hand it a goal and walk away: it asks what it needs to know, works on its own,
and checks with you, here or on your phone, before any big decision.

It comes two ways, both built on the same Rust core: a **Mac app** that lives in your menu bar,
and a **terminal hub and CLI**.

<p align="center">
  <img src="assets/app-light.png" alt="The Grandaddy app showing two questions from a Claude Code session" width="49%">
  <img src="assets/app-dark.png" alt="The Grandaddy app in dark mode, showing a session's activity and the message field" width="49%">
</p>

## Install

**The app.** Build it once (Rust, plus the Tauri bundler):

```sh
git clone https://github.com/Karang1908/grandaddy.git && cd grandaddy
cargo install tauri-cli --version "^2" --locked
./scripts/build-mac.sh        # → target/release/bundle/macos/Grandaddy.app and a .dmg
cp -R target/release/bundle/macos/Grandaddy.app /Applications/
```

The `grandaddy` command ships inside the app. **Settings → Command-line tool → Link** puts it on
your PATH. Then, in **Settings → Claude Code**, switch on **Control sessions from Grandaddy** and
**Answer prompts and questions here**.

**Just the CLI:**

```sh
cargo install --path crates/grandaddy
grandaddy install             # Claude Code's prompts and questions come to grandaddy
grandaddy terminal on         # `claude` in your shell runs through grandaddy, so you can drive it
```

Built and tested on macOS. The build is signed ad hoc, not notarized, so on another Mac you'll
need to right-click the app and choose Open the first time.

## Control a session

Every session you start with `claude` once **terminal control** is on (or with
`grandaddy run claude`) is fully yours from the app, the CLI, loops and other sessions:

- **Prompts and slash commands.** Type `/` in the message field for every command the session
  understands: Claude Code's own (`/compact`, `/btw`, `/goal`, `/review`, `/context`…) and your
  skills and commands from `~/.claude` and the project. Arrow keys and Tab pick one.
- **Mode.** Auto, Manual, Accept edits or Plan, switched the way you would: Shift+Tab, checked
  against what the session shows until it's right.
- **Model and effort.** `/model` and `/effort`, one click each. Claude Code keeps an effort change
  as that model's default for new sessions.
- **Interrupt.** Esc, from a button.
- **Never typed over.** If the session's input box already has text (a draft, or the prompt Claude
  puts back after you interrupt it), grandaddy asks before replacing it.

It looks and feels exactly like `claude`: your keyboard passes straight through, and closing the
window ends the session as it always did. `claude -p`, pipes and subcommands like `claude mcp` run
untouched. Sessions started before you turned it on, and ones in the Claude app, can still be
watched and answered; a message to one of those is copied and its window brought forward (iTerm,
Terminal and tmux sessions take it directly).

## What it does

**See everything.** Every Claude Code session, grouped by what it needs from you:
*Needs You*, *Working*, *Idle*. Pick one to see its activity as a conversation (your prompts,
its replies, the steps in between) with model, folder, branch and context size above it.
Long runs of steps fold up so the words stay readable. grandaddy warns you when a session goes
quiet, seems stuck in a loop, or edits a file another session is also editing.

**Answer from anywhere.** When a session asks a question, wants permission, or ends its turn by
asking you something, a popup appears at the top right of your screen, over whatever you're
doing, without taking the keyboard from you. Answer, allow, deny or reply right there, or choose
**Later**. In the sidebar, every session waiting on you is tinted yellow. Questions arrive with
their options and room to answer in your own words. Permission prompts get **Allow** and **Deny**. You can answer in the app, from the menu
bar, in the terminal hub, on your phone, or in the session itself. Whichever answer comes
first wins.

**Goals.** Describe an outcome ("add CSV export to the reports page, with tests"), pick a folder
and a model. The session asks its questions first, then works on its own, and stops to
ask before major decisions: changing scope, choosing between real alternatives, anything
destructive or that costs money. Those questions come to grandaddy and your phone. Goals track their session and show a summary when it's done.

**Loops.** A prompt on a schedule, sent to a session or to a fresh background session each
time. "Check CI and fix what's red, every 30 minutes." A loop waits while its session is busy,
and can stop itself once the reply contains a phrase you choose, or after a number of runs.

<p align="center">
  <img src="assets/app-git.png" alt="The Git tab: branch, pull request with checks and merge, changed files and a diff" width="80%">
</p>

**Git and pull requests.** Every session has a Git tab for its folder:
- branch and sync state, pull and push
- changed files with diffs, and Commit All
- branches: switch or create
- recent commits
- with GitHub's `gh` installed: open a pull request, see its checks and reviews, and merge it
  (squash, merge or rebase, with optional branch cleanup)

Merging asks first. Nothing ever force-pushes or force-deletes.

**Everything sleeps except the coding.** While a session is working or waiting on you, is due to
continue after a usage limit, or a loop is due within the hour, grandaddy keeps the Mac from
idle-sleeping, and lets go ten minutes after the last of those ends. The display still sleeps.
You also choose what closing the lid does:

- **Sleep:** as usual.
- **Keep everything on:** while any Claude session is open, the whole Mac keeps running, display
  included, so nothing locks.
- **Agents only:** while any Claude session is open, it keeps running and everything else rests. The display turns off,
  every app that isn't running a session is paused (browsers, terminals and call apps stay on a
  keep list you can edit), sound is muted and Low Power Mode turns on. Opening the lid undoes all
  of it. `grandaddy awake preview` shows which apps would pause right now.

Both lid modes need your password once, for a rule that allows exactly
`pmset -a disablesleep 0|1` and `pmset -b lowpowermode 0|1`. Every change is written down before
it's made, so if grandaddy crashes, the next start, or a watchdog the moment it dies, resumes
paused apps and puts sleep back. A restarting grandaddy takes those records over, so the old
one's watchdog leaves the new one's hold alone, and the hold is re-checked every few seconds.

**Continue after usage limits.** When a session stops on its 5-hour or weekly limit, grandaddy
shows when the limit resets, and with **Continue after a usage limit resets** on, it sends the
session "continue" (or your own message) as soon as it lifts, so the work carries on overnight
without you. It reads the exact reset time Claude Code records. Turn it off for one session from
the session itself.

<img src="assets/menubar.png" align="right" width="300" alt="The menu bar panel: what's waiting, sessions, and staying awake">

**From the menu bar.** Click the grandaddy icon for a panel with what's waiting (Allow and Deny
right there), every session with a field to message it, and whether it's holding your Mac awake.
The icon shows a count when something needs you. Closing the window keeps grandaddy running up
there, and **Open at login** starts it there.

**On your phone.** Two ways, and you can use both:
- **ntfy**: notifications with **Allow / Deny** buttons, wherever you are. Set it up with
  `grandaddy phone setup` or in Settings. Prompts pass through the ntfy server, so for anything
  sensitive use your own: `grandaddy phone setup https://ntfy.example.com`.
- **The phone page**: on the same Wi-Fi, scan the QR code in Settings. You can answer questions,
  follow goals and message sessions from your phone's browser. The link carries a private key.

**Rules.** Finer control than auto-yes, in Claude Code's `Tool(pattern)` syntax:

```sh
grandaddy rules add yes "Bash(npm test*)"    # approve automatically
grandaddy rules add ask "Bash(git push*)"    # always ask, even with auto-yes on
grandaddy rules add no  "Bash(rm -rf /*)"    # refuse
```

Questions are never answered by a rule or by auto-yes.

**Experimental: sessions that talk to each other.** Turn it on in **Settings → Experimental**
(or `grandaddy mesh on` and `grandaddy mcp install`) to add the grandaddy MCP server to Claude
Code. A session can then list and read other sessions, message the controllable ones, and start
helpers on any model, waiting for their results. A planner on one model can hand work to
sessions on others.

## The terminal hub

`grandaddy` with no arguments opens the same view in your terminal:

```
     _.---._
  ~ /       \ ~    grandaddy
 ~ | (◠)-(◠) | ~   Manage your vibe coding sessions. All in one place.
    \  ⌒⌒⌒  /
     `-...-'       ● 1 needs you   ● 2 working   ● 1 idle   today 3.1M tokens
──────────────────────────────────────────────────────────────────────────────────────────
 SESSIONS  4                        │ Fix flaky login test
  ● Fix flaky login test  working   │ Claude Code  ·  opus-5-5  ·  pid 41822
› ● Reports CSV export  needs you   │ ~/code/api  ·  fix/flaky-login  ·  3 changed
  ● Release pipeline  idle         │ ● working for 4m  ·  context 84k
```

| key | does |
|---|---|
| `↑` `↓` / `j` `k` | move · `tab` next session waiting on you |
| `y` / `n` | allow / deny a permission prompt |
| `1`–`9`, `enter`, `t` | answer a question (pick · send a multi-select · type your own) |
| `p` | send the session a prompt |
| `o` | jump to its window (attach, for background sessions) |
| `a` / `A` | auto-yes for this session / every session |
| `s` · `x x` | start a background session · stop one |
| `h` · `u` | past sessions · token usage |
| `m` · `i` · `?` · `q` | mute notifications · connect Claude Code · help · quit |

## Commands

```
grandaddy ls | watch [session] | pending
grandaddy approve | deny [session]
grandaddy answer [session] 2 1,3 "my own words"
grandaddy terminal on|off                        `claude` in your shell runs through grandaddy
grandaddy run claude [args…]                     one session grandaddy can control
grandaddy send [session] <prompt or /command> [--open] [--replace]
grandaddy mode [session] [auto|manual|acceptEdits|plan]
grandaddy interrupt [session] · commands [session]
grandaddy open | stop [session]
grandaddy start <prompt> [--in DIR] [--worktree [NAME]] [--model M]
grandaddy goal <what you want> [--in DIR] [--model M] [--terminal] · goal ls | rm
grandaddy loop add <prompt> --every 30m [--session S | --in DIR] [--until TEXT] [--max N]
grandaddy loop ls | pause | resume | rm <id>
grandaddy git [session] · git log | branches | prs
grandaddy git commit <message> | push | pull | branch <name> | switch <name>
grandaddy git pr <title> [--draft] · git merge <number> [--squash|--merge|--rebase] [--delete-branch]
grandaddy auto on|off [session|--all] · rules [add <rule> | rm <n>]
grandaddy install | uninstall                    Claude Code's approval hook
grandaddy awake on|off · awake lid off|full|agents · awake preview
grandaddy continue on|off ["message"] · continue skip|unskip [session]
grandaddy phone setup [server] | test | off · web on|off|url
grandaddy mesh on|off · mcp install | remove     (experimental)
grandaddy daemon                                 loops, staying awake and the phone page, without the app
grandaddy history [search] · resume <id> [prompt] · usage [days] · log [n]
```

A session is a pid, name, background id, session-id prefix or folder. Leave it out to mean the
session in the current folder, or the only one that's waiting.

## How it works

- **Sessions.** From `claude agents --json`, the transcripts in `~/.claude/projects`, and the
  Claude desktop app's session records.
- **Answering.** Claude Code runs `grandaddy hook` before it shows a permission dialog or asks a
  question. The hook checks your rules and auto-yes; if neither decides, it holds the request in
  `~/.grandaddy/pending/` for an answer from grandaddy or your phone while Claude's own dialog
  stays up, and whichever is answered first wins. Installing the hook keeps the rest of
  `~/.claude/settings.json` and writes a `.grandaddy.bak` copy first.
- **Usage limits.** A session's transcript records each limit stop with its exact reset time
  (`quotaLimits.resetsAt`). Auto-continue acts once per limit, 90 seconds after it lifts, and only
  in sessions it can type into or resume.
- **Controlling sessions.** `grandaddy run` starts `claude` inside a terminal it owns, passes your
  keyboard through untouched, and keeps a copy of the screen (a vt100 emulator). That copy is how
  it reads the mode, the effort and what's in the input box, and how it checks that a message was
  submitted. Messages arrive as a paste, slash commands as typing, then Return. Terminal control is
  a short `claude` function in `~/.zshrc` (and `~/.bashrc` if you have one), between marker
  comments, removed by `grandaddy terminal off`.
- **Code.**
  - `crates/grandaddy-core` does the work: sessions, hook, session control, commands, rules,
    goals, loops, git, staying awake, the phone page and the MCP server.
  - `crates/grandaddy` is the CLI and the terminal hub (ratatui).
  - `app/` is the Tauri app: Rust commands in `src-tauri`, plain HTML, CSS and JS in `ui`.
    Loops, staying awake and the phone page run inside the app, or under `grandaddy daemon`,
    never both.

## Limits

- Claude Code only, for now.
- Driving a session (mode, model, effort, commands, interrupt, input-box checks) needs it to have
  started under grandaddy: with terminal control on, or `grandaddy run claude`. Others take
  messages through iTerm, Terminal or tmux, or from your clipboard, and auto-continue sends you a
  notification instead.
- Mode and effort are read from Claude Code's footer, so a future change to its wording needs a
  grandaddy update. The effort is shown only while Claude Code shows it.
- Commands that open a menu in the session (`/rewind`, `/mcp`, `/config`…) are best finished in its
  own window.
- Pull requests need GitHub and the `gh` command, signed in.
- The phone page is plain HTTP on your local network. Use it on networks you trust.

## License

[MIT](LICENSE)

<br>
<p align="center">
  <img src="assets/icon.png" width="72" alt="grandaddy icon"><br>
  <sub>grandaddy keeps an eye on your sessions, so you don't have to.</sub>
</p>
