<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `BashTool::execute` *(src/tools/bash.rs)*

**Purpose:** Execute a shell command, capture output, enforce safety.
**Preconditions:** `params.command` is present.
**Postconditions:** Returns `Ok(ToolResult)` even for non-zero exit codes (LLM needs the error to self-correct).

```
FUNCTION BashTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  command ← params["command"] as String  // InvalidArgs if missing
  cancel ← ctx.cancel

  // Safety: check deny patterns (substring match)
  FOR EACH pattern IN self.deny_patterns
    IF command contains pattern THEN
      RETURN Err(Failed("Command blocked by safety policy: contains '{pattern}'"))
    END IF
  END FOR

  // Optional confirmation callback
  IF self.confirm_fn defined AND NOT self.confirm_fn(command) THEN
    RETURN Err(Failed("Command was not confirmed by the user."))
  END IF

  // Build subprocess: bash -c "{command}"
  cmd ← Command("bash", ["-c", command])
  IF self.cwd defined THEN cmd.current_dir(self.cwd) END IF
  cmd.stdout(piped), cmd.stderr(piped)

  // Race: cancellation vs timeout vs command completion
  result ← SELECT {
    cancel.cancelled()          → RETURN Err(Cancelled)
    sleep(self.timeout)         → RETURN Err(Failed("Command timed out after {N}s"))
    cmd.output()                → result  // may be Err if spawn failed
  }

  output ← result  // Err(io) → Err(Failed("Failed to execute: {e}"))

  stdout ← output.stdout as utf8 (lossy)
  stderr ← output.stderr as utf8 (lossy)

  // Truncate at limit
  IF stdout.len > self.max_output_bytes THEN
    stdout ← stdout[0..max_output_bytes] + "\n... (output truncated)"
  END IF
  IF stderr.len > self.max_output_bytes THEN
    stderr ← stderr[0..max_output_bytes] + "\n... (output truncated)"
  END IF

  exit_code ← output.exit_code OR -1

  text ←
    IF stderr is empty THEN
      "Exit code: {exit_code}\n{stdout}"
    ELSE
      "Exit code: {exit_code}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
    END IF

  // Always Ok — non-zero exit is NOT a ToolError
  RETURN Ok(ToolResult {
    content: [Text(text)],
    details: { exit_code, success: exit_code == 0 }
  })

END FUNCTION
```

---
