import type { Plugin } from "@opencode-ai/plugin"
import { randomUUID } from "node:crypto"
import { mkdir, open, rename, unlink } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"

type PetState =
  | "ready"
  | "thinking"
  | "working"
  | "waiting"
  | "done"
  | "error"
  | "offline"

const importantStates = new Set<PetState>(["waiting", "done", "error"])

const PulseDeckPet: Plugin = async () => {
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

  const writeState = async (
    state: PetState,
    options: { newTask?: boolean; heartbeat?: boolean } = {},
  ) => {
    const now = Date.now()
    if (options.newTask) taskID = randomUUID()

    const importantEdge = importantStates.has(state) && state !== current
    if (importantEdge) eventID += 1
    if (state === current && !options.heartbeat) return
    if (state === current && options.heartbeat && now - lastWrite < 60_000) return

    current = state
    lastWrite = now
    const directory = dirname(target)
    await mkdir(directory, { recursive: true, mode: 0o700 })
    const temporary = join(directory, `.opencode-pet.${process.pid}.${randomUUID()}.tmp`)
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

  await publish("ready")

  return {
    "chat.message": async () => publish("thinking", { newTask: true }),
    "tool.execute.before": async () => publish("working"),
    "permission.ask": async () => publish("waiting"),
    event: async ({ event }) => {
      const type = event.type as string
      if (type === "server.connected") {
        await publish("ready")
      } else if (type === "session.idle") {
        await publish("done")
      } else if (type === "session.error") {
        await publish("error")
      } else if (
        type === "permission.asked" ||
        type === "permission.v2.asked" ||
        type === "question.asked" ||
        type === "question.v2.asked"
      ) {
        await publish("waiting")
      } else if (type === "global.disposed" || type === "server.instance.disposed") {
        await publish("offline")
      } else if (type === "session.status") {
        const status = (event as { properties?: { status?: { type?: string } } }).properties
          ?.status?.type
        if (status === "idle") {
          await publish("done")
        } else if (status === "busy") {
          await publish(current === "working" ? "working" : "thinking", {
            heartbeat: true,
          })
        }
      } else if (
        type === "message.part.updated" &&
        (current === "thinking" || current === "working")
      ) {
        await publish(current, { heartbeat: true })
      }
    },
    dispose: async () => publish("offline"),
  }
}

export default PulseDeckPet
