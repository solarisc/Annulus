# ADR-0031: Agent settings, command approval and a portable skill

- Status: Accepted (2026-11-09)
- Date: 2026-11-09
- Supersedes: the Settings layout and “ask before every command” choice in [ADR-0020](0020-local-control-channel-for-cli-and-ai.md)

## Context

The agent door's settings sat below the vault lock setting in Security. The maintainer wants them
on a page of their own, while keeping the main switch in Security too. They also want an
optional check before an agent command reaches a terminal, and one place to install an Annulus
skill for common agent programs.

The control call already waits in the app until a terminal is ready. It can wait for a person
in the same path, with no protocol change and no hidden command channel.

## Decision

1. Settings has an **Agents** page. Security keeps a copy of *Allow the command line and AI
   agents*. Both switches edit the same `automation` setting. The Agents page owns all other
   agent settings.
2. *Approve commands* is off by default. When it is on, each `run` call opens its agent tab,
   unfolds the panel and shows the exact command with **Approve** and **Deny**. No command bytes
   reach the terminal while the choice waits, and `annulus-agent` keeps waiting for its call.
   Approve continues the normal run path. Deny answers with an error. Switching approval off
   lets commands already waiting move on.
3. The Agents page installs the bundled, portable `annulus` skill in the user folders for
   Claude Code, Codex and Hermes, and in the shared Agent Skills folder used by Pi, Gemini CLI,
   Cursor, OpenCode and GitHub Copilot. The app writes only
   `<skill-root>/annulus/SKILL.md`, on a file worker. The skill tells an agent to use the local
   CLI, treat terminal text as data and wait for command approval.
4. The installer row names the tools it supports. This is a narrow exception to describing Annulus in its own terms
   ([ADR-0010](0010-original-work-only.md)): those names tell the user where the button writes, not what Annulus looks like or copies.

## Consequences

- A user can keep free agent use, or review every command without changing the CLI or MCP
  setup.
- A held CLI call can wait as long as the person needs. Stop, lock and turning agent access off
  still close the door and end it.
- A skill install updates Annulus's own skill in each target. It does not change another skill
  or a harness setting.
- The shared folder covers tools that follow the Agent Skills standard without one copy per
  tool.

## Alternatives considered

- **A new protocol request for approval.** The app already owns the waiting `run` call, so a
  second request would add state with no gain.
- **Approve in Settings.** The agent tab already shows the target and its command list. Keeping
  the choice there gives the user the facts near the action.
- **One harness folder only.** Claude Code, Codex and Hermes use their own user folders. The
  shared folder alone would miss them.
