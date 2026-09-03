import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"
import { randomUUID } from "node:crypto"
import { mkdir, open, rename, unlink } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"

type PetState =
  | "offline"
  | "ready"
  | "thinking"
  | "working"
  | "coding"
  | "waiting"
  | "confirm"
  | "cancelled"
  | "aborted"
  | "error"
  | "done"

const activeStates = new Set<PetState>(["thinking", "working", "coding"])
const importantStates = new Set<PetState>([
  "waiting",
  "confirm",
  "cancelled",
  "aborted",
  "error",
  "done",
])

function toolState(toolName: string): PetState {
  return toolName === "edit" || toolName === "write" || toolName === "apply_patch"
    ? "coding"
    : "working"
}

export default function pulsedeckPet(pi: ExtensionAPI) {
  const uid = typeof process.getuid === "function" ? process.getuid() : 0
  const target =
    process.env.PULSEDECK_PET_STATE_FILE ??
    (process.env.XDG_RUNTIME_DIR
      ? join(process.env.XDG_RUNTIME_DIR, "pulsedeck", "codex-pet.json")
      : join(tmpdir(), `pulsedeck-${uid}`, "pulsedeck", "codex-pet.json"))

  let taskID = "none"
  let eventID = 0
  let current: PetState = "offline"
  let lastWrite = 0
  let writes = Promise.resolve()
  let stateBeforePrompt: PetState = "ready"
  let promptActive = false
  let pendingTerminalState: "aborted" | "error" | undefined

  const writeState = async (
    state: PetState,
    options: { newTask?: boolean; heartbeat?: boolean } = {},
  ) => {
    const now = Date.now()
    if (options.newTask) taskID = randomUUID()

    const importantEdge = importantStates.has(state) && state !== current
    if (importantEdge) eventID += 1
    if (state === current && !options.newTask && !options.heartbeat) return
    if (state === current && options.heartbeat && now - lastWrite < 60_000) return

    current = state
    lastWrite = now
    const directory = dirname(target)
    await mkdir(directory, { recursive: true, mode: 0o700 })
    const temporary = join(directory, `.pi-pet.${process.pid}.${randomUUID()}.tmp`)
    const body = `${JSON.stringify({
      version: 2,
      task_id: taskID,
      event_id: String(eventID),
      state,
      timestamp_ms: now,
    })}\n`

    try {
      const file = await open(temporary, "wx", 0o600)
      try {
        await file.writeFile(body, "utf8")
      } finally {
        await file.close()
      }
      await rename(temporary, target)
    } finally {
      await unlink(temporary).catch(() => {})
    }
  }

  const publish = (
    state: PetState,
    options?: { newTask?: boolean; heartbeat?: boolean },
  ) => {
    writes = writes.then(() => writeState(state, options)).catch(() => {})
    return writes
  }

  pi.on("session_start", async () => {
    promptActive = false
    pendingTerminalState = undefined
    await publish("ready")
  })
  pi.on("before_agent_start", async () => {
    promptActive = false
    pendingTerminalState = undefined
    await publish("thinking", { newTask: true })
  })
  pi.on("agent_start", async () => {
    pendingTerminalState = undefined
    await publish("thinking")
  })
  pi.on("tool_execution_start", async (event) => publish(toolState(event.toolName)))
  pi.on("tool_execution_update", async () => {
    if (activeStates.has(current)) {
      await publish(current, { heartbeat: true })
    }
  })
  pi.on("tool_execution_end", async () => {
    if (activeStates.has(current)) {
      await publish(current, { heartbeat: true })
    }
  })
  pi.on("message_update", async (event) => {
    if (event.assistantMessageEvent.type === "error") {
      pendingTerminalState = event.assistantMessageEvent.reason === "aborted" ? "aborted" : "error"
    }
    if (activeStates.has(current)) {
      await publish(current, { heartbeat: true })
    }
  })
  pi.on("ui_prompt_start", async (event) => {
    if (!promptActive) {
      stateBeforePrompt = activeStates.has(current) ? current : "ready"
      promptActive = true
    }
    await publish(event.kind === "confirm" ? "confirm" : "waiting")
  })
  pi.on("ui_prompt_end", async () => {
    const next = promptActive ? stateBeforePrompt : "ready"
    promptActive = false
    await publish(next === "waiting" ? "ready" : next)
  })
  pi.on("agent_settled", async () => {
    const state = pendingTerminalState ?? "done"
    pendingTerminalState = undefined
    await publish(state)
  })
  pi.on("session_shutdown", async () => {
    promptActive = false
    pendingTerminalState = undefined
    await publish("offline")
  })
}
