# Prompt for the session that leads this round

Paste this into a new session. It needs nothing from the session that wrote the handover.

```text
You are leading the move of Buwiz Bot's window control off our NSH fork's old line and onto the plug.

Repository: https://github.com/buwiz-bot/buwiz-bot, branch feat/bot-agent-plug.
Read handover/agent-plug/GOAL.md, handover/agent-plug/PLAN.md and handover/agent-plug/TASKS.md completely,
then everything GOAL.md lists under "Read first", before you change anything. The decisions under
"Decisions already made" in PLAN.md are final; do not ask about them again.

Do Stage 0 of PLAN.md yourself in this session: the probe. Build the candidate NSH line for the Bot, compile
the Bot and the plug against it, fill the gap table from what the compiler says, and settle the open items
PLAN.md marks as inferred. Follow GOAL.md's rules and its Allowed / Not allowed lists exactly.

When Stage 0 passes its done list, stop and report: go or stop, the gap table from compiling, the
branch-layout recommendation with its evidence, every command you ran with its output, and your answer or
recommendation for each item under "Open decisions for the owner". If Stage 0 shows the move is not small,
stop there and report that instead, as GOAL.md says.

Stages 1 to 6 start only after the owner has answered the open decisions.
```

Helper machines (Devin sessions for macOS, Linux and Windows) are named in TASKS.md by the system each check
needs. The owner supplies their session links.
