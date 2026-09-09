import { expect, test } from "@playwright/test";

import { installTauriStub } from "./tauri-stub.js";

/** A library with two folders and a few tracks, as the backend reports it. */
const FOLDERS = [
  { id: 1, path: "/music" },
  { id: 2, path: "/more music" },
];

const STATS = { track_count: 3, total_duration_ms: 754000 };

/** The library as the backend reports it, already sorted. */
const ROWS = [
  row(1, "Indie", "Alpha", "First", "One", 1),
  row(2, "Indie", "Alpha", "First", "Two", 2),
  row(3, "Rock", "Beta", "Second", "Three", 1),
];

function row(id, genre, artist, album, title, trackNumber, overrides = {}) {
  return {
    id,
    path: `/music/${id}.mp3`,
    genre,
    artist,
    album,
    title,
    track_number: trackNumber,
    disc_number: 1,
    rating: null,
    grouping: null,
    duration_ms: 60000,
    ...overrides,
  };
}

/** One playlist holding the two Indie tracks. */
const PLAYLISTS = [{ id: 1, name: "Favourites", tracks: [ROWS[0], ROWS[1]] }];

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
    library_rows: ROWS,
    list_playlists: [],
  });
  await page.goto("/");
  await expect(page.getByText("3 tracks")).toBeVisible();
}

/** The commands the app has sent so far. */
function calls(page) {
  return page.evaluate(() => window.__SATSUMA_TEST__.calls());
}

/** Loads the app with a playlist already there and open. */
async function openWithPlaylist(page, playlists = PLAYLISTS) {
  await page.addInitScript(installTauriStub, {
    ping: "satsuma test",
    list_folders: FOLDERS,
    library_stats: STATS,
    library_rows: ROWS,
    list_playlists: playlists,
  });
  await page.goto("/");
  await expect(page.getByRole("button", { name: playlists[0].name })).toBeVisible();
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
    .toEqual([
      "library_rows",
      "library_stats",
      "list_folders",
      "list_playlists",
      "ping",
      "player_state",
      "start_scan",
    ]);
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
  await expect(page.getByTitle("Play", { exact: true })).toBeVisible();
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

test("the tree groups the library by genre, artist and album", async ({ page }) => {
  await open(page);
  const tree = page.locator("ul.tree").first();
  await expect(tree.getByRole("button", { name: "Indie" })).toBeVisible();
  await expect(tree.getByRole("button", { name: "Rock" })).toBeVisible();
  // The levels below stay closed until they are opened.
  await expect(page.getByRole("button", { name: "Alpha" })).toHaveCount(0);
});

test("a branch opens and closes", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Indie" }).click();
  await expect(page.getByRole("button", { name: "Alpha" })).toBeVisible();

  await page.getByRole("button", { name: "Alpha" }).click();
  await expect(page.getByRole("button", { name: "First" })).toBeVisible();

  await page.getByRole("button", { name: "First" }).click();
  await expect(page.getByRole("button", { name: "1. One" })).toBeVisible();

  await page.getByRole("button", { name: "Indie" }).click();
  await expect(page.getByRole("button", { name: "Alpha" })).toHaveCount(0);
});

test("a branch says how many tracks are under it", async ({ page }) => {
  await open(page);
  await expect(page.locator(".tree-node", { hasText: "Indie" }).first()).toContainText(
    "2",
  );
});

test("a track plays on a double click, not on a single one", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Indie" }).click();
  await page.getByRole("button", { name: "Alpha" }).click();
  await page.getByRole("button", { name: "First" }).click();
  await clearCalls(page);

  // A single click does not play: that would restart the track on the
  // second half of a double click.
  await page.getByRole("button", { name: "1. One" }).click();
  await expect(page.locator("body")).toBeVisible();
  expect(await calls(page)).toEqual([]);

  // The rest of the album follows it, rather than the queue holding one
  // track and stopping.
  await page.getByRole("button", { name: "1. One" }).dblclick();
  await expect.poll(() => calls(page)).toEqual([
    { command: "play_tracks", args: { ids: [1, 2], startId: 1 } },
  ]);
});

test("choosing the second track plays the album from there", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Indie" }).click();
  await page.getByRole("button", { name: "Alpha" }).click();
  await page.getByRole("button", { name: "First" }).click();
  await clearCalls(page);

  await page.getByRole("button", { name: "2. Two" }).dblclick();
  await expect.poll(() => calls(page)).toEqual([
    { command: "play_tracks", args: { ids: [1, 2], startId: 2 } },
  ]);
});

test("double-clicking a track plays it exactly once", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Indie" }).click();
  await page.getByRole("button", { name: "Alpha" }).click();
  await page.getByRole("button", { name: "First" }).click();
  await clearCalls(page);

  await page.getByRole("button", { name: "1. One" }).dblclick();
  await expect.poll(() => calls(page)).toEqual([
    { command: "play_tracks", args: { ids: [1, 2], startId: 1 } },
  ]);
});

test("double-clicking a branch plays everything under it", async ({ page }) => {
  await open(page);
  await clearCalls(page);
  await page.getByRole("button", { name: "Indie" }).dblclick();
  await expect.poll(() => calls(page)).toContainEqual({
    command: "play_tracks",
    args: { ids: [1, 2], startId: null },
  });
});

test("the plus button queues everything under a branch", async ({ page }) => {
  await open(page);
  await clearCalls(page);
  await page.getByTitle("Add Indie to the queue").click();
  await expect.poll(() => calls(page)).toEqual([
    { command: "enqueue_tracks", args: { ids: [1, 2] } },
  ]);
});

test("the filter narrows the tree and clearing it restores everything", async ({
  page,
}) => {
  await open(page);
  await page.getByLabel("Filter the library").fill("beta");
  await expect(page.getByRole("button", { name: "Rock" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Indie" })).toHaveCount(0);

  await page.getByLabel("Filter the library").fill("");
  await expect(page.getByRole("button", { name: "Indie" })).toBeVisible();
});

test("a filter matching nothing says so", async ({ page }) => {
  await open(page);
  await page.getByLabel("Filter the library").fill("zzz");
  await expect(page.getByText("Nothing matches that filter.")).toBeVisible();
});

test("the grouping can be changed", async ({ page }) => {
  await open(page);
  await page.getByLabel("Group the library by").selectOption("Artist / Album");
  await expect(page.getByRole("button", { name: "Alpha" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Indie" })).toHaveCount(0);

  await page.getByLabel("Group the library by").selectOption("Album");
  await expect(page.getByRole("button", { name: "First" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Alpha" })).toHaveCount(0);
});

test("what is open stays open when the library is scanned again", async ({
  page,
}) => {
  await open(page);
  await page.getByRole("button", { name: "Indie" }).click();
  await expect(page.getByRole("button", { name: "Alpha" })).toBeVisible();

  await emit(page, "library://scan-finished", {
    status: "finished",
    added: 1,
    updated: 0,
    removed: 0,
    failed: 0,
    unreachable: 0,
    emptied: 0,
  });

  await expect(page.getByRole("button", { name: "Alpha" })).toBeVisible();
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

test("a playlist shows its tracks in the columns the requirements ask for", async ({
  page,
}) => {
  await openWithPlaylist(page);
  const headings = page.locator("th");
  await expect(headings).toHaveText([
    "Genre",
    "Artist",
    "Track",
    "Title",
    "Album",
    "Duration",
    "Rating",
    "Grouping",
  ]);
  await expect(page.locator(".playlist-row")).toHaveCount(2);
  await expect(page.getByText("2 tracks [2:00]")).toBeVisible();
});

test("without a playlist the panel says what to do", async ({ page }) => {
  await open(page);
  await expect(
    page.getByText("Make a playlist, then add tracks to it from the library."),
  ).toBeVisible();
});

test("a new playlist is created and opened", async ({ page }) => {
  await open(page);
  await page.evaluate(() => window.__SATSUMA_TEST__.reply("create_playlist", 3));
  await clearCalls(page);
  await page.getByTitle("New playlist").click();
  await expect
    .poll(() => calls(page))
    .toContainEqual({ command: "create_playlist", args: { name: "Playlist 1" } });
});

test("a tab is renamed by double-clicking it", async ({ page }) => {
  await openWithPlaylist(page);
  await page.getByRole("button", { name: "Favourites" }).dblclick();
  const field = page.getByLabel("Playlist name");
  await expect(field).toBeVisible();
  await field.fill("Loud ones");
  await clearCalls(page);
  await field.press("Enter");
  await expect
    .poll(() => calls(page))
    .toContainEqual({ command: "rename_playlist", args: { id: 1, name: "Loud ones" } });
});

test("escape gives up on a rename", async ({ page }) => {
  await openWithPlaylist(page);
  await page.getByRole("button", { name: "Favourites" }).dblclick();
  await clearCalls(page);
  await page.getByLabel("Playlist name").press("Escape");
  await expect(page.getByRole("button", { name: "Favourites" })).toBeVisible();
  expect(await calls(page)).toEqual([]);
});

test("a playlist is deleted", async ({ page }) => {
  await openWithPlaylist(page);
  await clearCalls(page);
  await page.getByTitle("Delete Favourites").click();
  await expect
    .poll(() => calls(page))
    .toContainEqual({ command: "delete_playlist", args: { id: 1 } });
});

test("clicking a heading sorts, then reverses, then gives the order back", async ({
  page,
}) => {
  const unsorted = [
    { id: 1, name: "Mixed", tracks: [row(2, "Rock", "Zed", "Later", "Beta", 2), row(1, "Indie", "Alpha", "First", "Alpha", 1)] },
  ];
  await openWithPlaylist(page, unsorted);
  const titles = page.locator(".playlist-row td:nth-child(4)");
  await expect(titles).toHaveText(["Beta", "Alpha"]);

  await page.getByRole("button", { name: "Title" }).click();
  await expect(titles).toHaveText(["Alpha", "Beta"]);

  await page.getByRole("button", { name: /^Title/ }).click();
  await expect(titles).toHaveText(["Beta", "Alpha"]);

  await page.getByRole("button", { name: /^Title/ }).click();
  await expect(titles).toHaveText(["Beta", "Alpha"]);
});

test("clicking a star rates the track and clicking it again clears it", async ({
  page,
}) => {
  const rated = [
    { id: 1, name: "Rated", tracks: [row(1, "Indie", "Alpha", "First", "One", 1, { rating: 3 })] },
  ];
  await openWithPlaylist(page, rated);
  await clearCalls(page);

  await page.getByTitle("4 stars").click();
  await expect
    .poll(() => calls(page))
    .toContainEqual({ command: "set_rating", args: { id: 1, stars: 4 } });

  await clearCalls(page);
  await page.getByTitle("Remove the rating").click();
  await expect
    .poll(() => calls(page))
    .toContainEqual({ command: "set_rating", args: { id: 1, stars: null } });
});

test("double-clicking a row plays the playlist from it", async ({ page }) => {
  await openWithPlaylist(page);
  await clearCalls(page);
  await page.locator(".playlist-row").nth(1).dblclick();
  await expect
    .poll(() => calls(page))
    .toContainEqual({ command: "play_tracks", args: { ids: [1, 2], startId: 2 } });
});

test("the track that is playing stands out", async ({ page }) => {
  await openWithPlaylist(page);
  await emit(
    page,
    "player://state",
    playerState({
      status: "playing",
      track: { id: 2, title: "Two", artist: null, album: null, duration_ms: 60000 },
    }),
  );
  await expect(page.locator(".playlist-row.is-playing")).toHaveCount(1);
});

test("the library button adds to the open playlist rather than the queue", async ({
  page,
}) => {
  await openWithPlaylist(page);
  await clearCalls(page);
  await page.getByTitle("Add Indie to the playlist").click();
  await expect
    .poll(() => calls(page))
    .toContainEqual({ command: "add_to_playlist", args: { id: 1, ids: [1, 2] } });
});

test("with no playlist the library button still fills the queue", async ({ page }) => {
  await open(page);
  await clearCalls(page);
  await page.getByTitle("Add Indie to the queue").click();
  await expect
    .poll(() => calls(page))
    .toContainEqual({ command: "enqueue_tracks", args: { ids: [1, 2] } });
});

test("a column is resized by dragging its grip", async ({ page }) => {
  await openWithPlaylist(page);
  const heading = page.locator("th").first();
  const before = await heading.evaluate((element) => element.getBoundingClientRect().width);

  const grip = heading.locator(".column-grip");
  const box = await grip.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 60, box.y + box.height / 2, { steps: 6 });
  await page.mouse.up();

  const after = await heading.evaluate((element) => element.getBoundingClientRect().width);
  expect(after).toBeGreaterThan(before + 30);
});

test("the panels switch", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Now playing" }).click();
  await expect(page.getByRole("heading", { name: "Now playing" })).toBeVisible();
  await page.getByRole("button", { name: "Playlists" }).click();
  await expect(page.getByRole("heading", { name: "Playlists" })).toBeVisible();
});
