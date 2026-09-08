import { expect, test } from "@playwright/test";

import { installTauriStub } from "./tauri-stub.js";

/** A library with two folders and a few tracks, as the backend reports it. */
const FOLDERS = [
  { id: 1, path: "/music" },
  { id: 2, path: "/more music" },
];

const STATS = { track_count: 3, total_duration_ms: 754000 };

function playerState(overrides = {}) {
  return {
    status: "stopped",
    track: null,
    position_ms: 0,
    volume: 1,
    shuffle: false,
    repeat: "off",
    stop_after_current: false,
    queue_length: 0,
    seeks_applied: 0,
    error: null,
    ...overrides,
  };
}

/**
 * Loads the app against the stubbed host, with a library already scanned.
 */
async function open(page) {
  // Seeded before the page loads, so the commands Elm sends on startup get
  // real answers rather than null.
  await page.addInitScript(installTauriStub, {
    ping: "satsuma test",
    list_folders: FOLDERS,
    library_stats: STATS,
  });
  await page.goto("/");
  await expect(page.getByText("3 tracks")).toBeVisible();
}

/** The commands the app has sent so far. */
function calls(page) {
  return page.evaluate(() => window.__SATSUMA_TEST__.calls());
}

/** Forgets the commands sent so far, so the next assertion starts clean. */
function clearCalls(page) {
  return page.evaluate(() => window.__SATSUMA_TEST__.clearCalls());
}

function emit(page, event, payload) {
  return page.evaluate(
    ([event, payload]) => window.__SATSUMA_TEST__.emit(event, payload),
    [event, payload],
  );
}

test("subscribes to the events the backend sends", async ({ page }) => {
  await page.addInitScript(installTauriStub);
  await page.goto("/");
  await expect
    .poll(() =>
      page.evaluate(() => [
        window.__SATSUMA_TEST__.listensTo("library://scan-progress"),
        window.__SATSUMA_TEST__.listensTo("library://scan-finished"),
        window.__SATSUMA_TEST__.listensTo("player://state"),
      ]),
    )
    .toEqual([true, true, true]);
});

test("asks for the library and the player state on startup", async ({ page }) => {
  await page.addInitScript(installTauriStub);
  await page.goto("/");
  await expect
    .poll(async () => (await calls(page)).map((call) => call.command).sort())
    .toEqual(["library_stats", "list_folders", "ping", "player_state", "start_scan"]);
});

test("shows the library, its folders and that the backend answered", async ({
  page,
}) => {
  await open(page);
  await expect(page.getByText("Backend: satsuma test")).toBeVisible();
  await expect(page.getByText("3 tracks · 12:34")).toBeVisible();
  await expect(page.getByTitle("/music")).toBeVisible();
  await expect(page.getByTitle("/more music")).toBeVisible();
});

test("Play all asks the backend to play the library", async ({ page }) => {
  await open(page);
  await clearCalls(page);
  await page.getByRole("button", { name: "Play all" }).click();
  await expect
    .poll(async () => (await calls(page)).map((call) => call.command))
    .toEqual(["play_library"]);
});

test("Queue all asks the backend to append the library", async ({ page }) => {
  await open(page);
  await clearCalls(page);
  await page.getByRole("button", { name: "Queue all" }).click();
  await expect
    .poll(async () => (await calls(page)).map((call) => call.command))
    .toEqual(["enqueue_library"]);
});

test("shows what is playing and how far in", async ({ page }) => {
  await open(page);
  await emit(
    page,
    "player://state",
    playerState({
      status: "playing",
      track: {
        id: 7,
        title: "Orange Sun",
        artist: "The Satsumas",
        album: "Citrus",
        duration_ms: 185000,
      },
      position_ms: 65000,
      queue_length: 3,
    }),
  );
  await expect(page.getByText("Orange Sun")).toBeVisible();
  await expect(page.getByText("The Satsumas")).toBeVisible();
  await expect(page.getByText("1:05")).toBeVisible();
  await expect(page.getByText("3:05")).toBeVisible();
});

test("the transport buttons send their commands", async ({ page }) => {
  await open(page);
  await emit(page, "player://state", playerState({ status: "playing" }));
  await clearCalls(page);

  await page.getByTitle("Previous").click();
  await page.getByTitle("Pause").click();
  await page.getByTitle("Next").click();
  await page.getByTitle("Stop", { exact: true }).click();

  await expect
    .poll(async () => (await calls(page)).map((call) => call.command))
    .toEqual([
      "player_previous",
      "player_play_pause",
      "player_next",
      "player_stop",
    ]);
});

test("play and pause swap with the reported status", async ({ page }) => {
  await open(page);
  await emit(page, "player://state", playerState({ status: "playing" }));
  await expect(page.getByTitle("Pause")).toBeVisible();
  await emit(page, "player://state", playerState({ status: "paused" }));
  await expect(page.getByTitle("Play")).toBeVisible();
});

test("shuffle and stop after this track send the opposite of the state", async ({
  page,
}) => {
  await open(page);
  await emit(page, "player://state", playerState({ shuffle: true }));
  await clearCalls(page);

  await page.getByTitle("Shuffle").click();
  await page.getByTitle("Stop after this track").click();

  await expect.poll(() => calls(page)).toEqual([
    { command: "player_set_shuffle", args: { shuffle: false } },
    { command: "player_set_stop_after_current", args: { stop: true } },
  ]);
});

test("repeat cycles off, playlist, track", async ({ page }) => {
  await open(page);
  const modes = ["off", "queue", "track"];
  const sent = [];
  for (const mode of modes) {
    await emit(page, "player://state", playerState({ repeat: mode }));
    await clearCalls(page);
    await page.getByTitle(/^Repeat/).click();
    await expect.poll(async () => (await calls(page)).length).toBe(1);
    sent.push((await calls(page))[0].args.repeat);
  }
  expect(sent).toEqual(["queue", "track", "off"]);
});

test("dragging the seek bar asks the backend to seek", async ({ page }) => {
  await open(page);
  await emit(
    page,
    "player://state",
    playerState({
      status: "playing",
      track: { id: 1, title: "T", artist: null, album: null, duration_ms: 100000 },
      position_ms: 1000,
    }),
  );
  await clearCalls(page);

  await page.locator("input.seek").fill("42000");

  await expect.poll(() => calls(page)).toEqual([
    { command: "player_seek", args: { positionMs: 42000 } },
  ]);
});

test("the seek bar shows where the user dropped it until the backend seeks", async ({
  page,
}) => {
  await open(page);
  const playing = playerState({
    status: "playing",
    track: { id: 1, title: "T", artist: null, album: null, duration_ms: 100000 },
    position_ms: 1000,
  });
  await emit(page, "player://state", playing);
  await page.locator("input.seek").fill("42000");

  // A tick that still reports the old position must not move the handle
  // back under the pointer.
  await emit(page, "player://state", { ...playing, position_ms: 1200 });
  await expect(page.locator("input.seek")).toHaveValue("42000");

  // Once the backend says it seeked, its own position takes over again.
  await emit(page, "player://state", {
    ...playing,
    position_ms: 42500,
    seeks_applied: 1,
  });
  await expect(page.locator("input.seek")).toHaveValue("42500");
});

test("the volume slider sends a fraction", async ({ page }) => {
  await open(page);
  await clearCalls(page);
  await page.locator("input.volume").fill("40");
  await expect.poll(() => calls(page)).toEqual([
    { command: "player_set_volume", args: { volume: 0.4 } },
  ]);
});

test("a playback failure is shown", async ({ page }) => {
  await open(page);
  await emit(
    page,
    "player://state",
    playerState({ error: "cannot play /music/gone.mp3" }),
  );
  await expect(page.getByText("cannot play /music/gone.mp3")).toBeVisible();
});

test("a failing command is shown", async ({ page }) => {
  await open(page);
  await page.evaluate(() =>
    window.__SATSUMA_TEST__.failWith("play_library", "the library is empty"),
  );
  await page.getByRole("button", { name: "Play all" }).click();
  await expect(page.getByText("the library is empty")).toBeVisible();
});

test("a failure that is not a string is still readable", async ({ page }) => {
  await open(page);
  await page.evaluate(() =>
    window.__SATSUMA_TEST__.failWith("play_library", { kind: "no device" }),
  );
  await page.getByRole("button", { name: "Play all" }).click();
  await expect(page.getByText('{"kind":"no device"}')).toBeVisible();
});

test("a finished scan is reported and the library asked for again", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() => {
    window.__SATSUMA_TEST__.reply("library_stats", {
      track_count: 5,
      total_duration_ms: 60000,
    });
    window.__SATSUMA_TEST__.clearCalls();
  });

  await emit(page, "library://scan-finished", {
    status: "finished",
    added: 2,
    updated: 0,
    removed: 1,
    failed: 0,
    unreachable: 0,
    emptied: 0,
  });

  await expect(page.getByText("Scan finished: 2 added, 1 removed")).toBeVisible();
  // The panel must ask for the library again, or it would keep showing the
  // counts from before the scan.
  await expect(page.getByText("5 tracks · 1:00")).toBeVisible();
  await expect
    .poll(async () => (await calls(page)).map((call) => call.command))
    .toContain("library_stats");
});

test("Rescan asks the backend to scan again", async ({ page }) => {
  await open(page);
  // Rescan stays disabled until the scan that runs at startup reports back.
  await expect(page.getByRole("button", { name: "Rescan" })).toBeDisabled();
  await emit(page, "library://scan-finished", {
    status: "finished",
    added: 0,
    updated: 0,
    removed: 0,
    failed: 0,
    unreachable: 0,
    emptied: 0,
  });
  await clearCalls(page);
  await page.getByRole("button", { name: "Rescan" }).click();
  await expect
    .poll(async () => (await calls(page)).map((call) => call.command))
    .toEqual(["start_scan"]);
});

test("scan progress is shown while a scan runs", async ({ page }) => {
  await open(page);
  await emit(page, "library://scan-progress", {
    scanned: 40,
    total: 100,
    path: "/music/a.mp3",
  });
  await expect(page.getByText("Scanning 40 / 100")).toBeVisible();
});

test("Add folder asks for a folder and adds the one that was picked", async ({
  page,
}) => {
  await open(page);
  await page.evaluate(() =>
    window.__SATSUMA_TEST__.reply("pick_folder", "/picked"),
  );
  await clearCalls(page);
  await page.getByRole("button", { name: "Add folder" }).click();
  await expect
    .poll(async () => (await calls(page)).map((call) => call.command))
    .toContain("add_folder");
});

test("removing a folder sends its id", async ({ page }) => {
  await open(page);
  await clearCalls(page);
  await page.getByTitle("Remove folder").first().click();
  await expect.poll(() => calls(page)).toContainEqual({
    command: "remove_folder",
    args: { id: 1 },
  });
});

test("the theme toggle cycles and survives a reload", async ({ page }) => {
  await open(page);
  await expect(page.locator(".app")).toHaveAttribute("data-theme", /light|dark/);
  await page.getByTitle("Switch theme").click();
  await expect(page.getByRole("button", { name: "Theme: Light" })).toBeVisible();
  await expect(page.locator(".app")).toHaveAttribute("data-theme", "light");

  await page.getByTitle("Switch theme").click();
  await expect(page.locator(".app")).toHaveAttribute("data-theme", "dark");

  await page.reload();
  await expect(page.getByRole("button", { name: "Theme: Dark" })).toBeVisible();
});

test("the panels switch", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Now playing" }).click();
  await expect(page.getByRole("heading", { name: "Now playing" })).toBeVisible();
  await page.getByRole("button", { name: "Playlists" }).click();
  await expect(page.getByRole("heading", { name: "Playlists" })).toBeVisible();
});
