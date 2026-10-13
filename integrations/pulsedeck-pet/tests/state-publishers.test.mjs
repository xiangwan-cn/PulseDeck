// Run with Node.js 22.18+ (native TypeScript stripping): node --test this-file
import test from "node:test"
import assert from "node:assert/strict"
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { spawnSync } from "node:child_process"
import openCodePet from "../opencode/pulsedeck-pet.ts"
import piPet from "../pi/pulsedeck-pet.ts"

const script = fileURLToPath(new URL("../scripts/pet_state.sh", import.meta.url))

async function fixture(run) {
  const directory = await mkdtemp(join(tmpdir(), "pulsedeck-hook-test-"))
  const previous = process.env.PULSEDECK_PET_STATE_FILE
  const target = join(directory, "state.json")
  process.env.PULSEDECK_PET_STATE_FILE = target
  try {
    await run({ directory, target, read: async () => JSON.parse(await readFile(target, "utf8")) })
  } finally {
    if (previous === undefined) delete process.env.PULSEDECK_PET_STATE_FILE
    else process.env.PULSEDECK_PET_STATE_FILE = previous
    await rm(directory, { recursive: true, force: true })
  }
}

function shell(action, directory, target, input = "{}") {
  const result = spawnSync("sh", [script, action], {
    cwd: directory,
    env: { ...process.env, PULSEDECK_PET_STATE_FILE: target },
    input,
    encoding: "utf8",
  })
  assert.equal(result.status, 0, result.stderr)
}

test("shell publishes offline and every new task without retaining input", async () => fixture(async ({ directory, target, read }) => {
  shell("start", directory, target, '{"prompt":"private-example"}')
  const first = await read()
  shell("start", directory, target)
  assert.notEqual((await read()).task_id, first.task_id)
  shell("offline", directory, target)
  assert.equal((await read()).state, "offline")
  assert.equal((await readFile(target, "utf8")).includes("private-example"), false)
}))

test("shell accepts a relative filename and recovers overflowing metadata", async () => fixture(async ({ directory, read }) => {
  await writeFile(join(directory, ".codex-pet-meta"), `task ${"9".repeat(80)} error ${"9".repeat(80)}\n`)
  shell("done", directory, "state.json")
  assert.equal((await read()).state, "done")
  assert.equal((await read()).event_id, "1")
}))

test("shell deduplicates repeated completion events", async () => fixture(async ({ directory, target, read }) => {
  shell("done", directory, target)
  const first = await read()
  shell("done", directory, target)
  assert.deepEqual(await read(), first)
}))

test("shell normalizes leading zeroes before arithmetic", async () => fixture(async ({ directory, target, read }) => {
  await writeFile(join(directory, ".codex-pet-meta"), "task 00008 error 00000\n")
  shell("done", directory, target)
  assert.equal((await read()).event_id, "9")
}))

for (const name of ["OpenCode", "pi"]) {
  async function publisher() {
    if (name === "OpenCode") {
      const hooks = await openCodePet({})
      return { start: () => hooks["chat.message"](), done: () => hooks.event({ event: { type: "session.idle" } }) }
    }
    const hooks = new Map()
    piPet({ on: (event, callback) => hooks.set(event, callback) })
    return { start: () => hooks.get("before_agent_start")(), done: () => hooks.get("agent_settled")() }
  }

  test(`${name} publishes consecutive thinking tasks`, async () => fixture(async ({ read }) => {
    const hooks = await publisher()
    await hooks.start()
    const first = await read()
    await hooks.start()
    assert.notEqual((await read()).task_id, first.task_id)
  }))

  test(`${name} retries a state after an unsuccessful write`, async () => fixture(async ({ target, read }) => {
    const hooks = await publisher()
    await hooks.start()
    await rm(target)
    // A directory at the destination makes the atomic rename fail.
    await mkdir(target)
    await hooks.done()
    await rm(target, { recursive: true })
    await hooks.done()
    assert.equal((await read()).state, "done")
    assert.equal((await read()).event_id, "1")
  }))
}
