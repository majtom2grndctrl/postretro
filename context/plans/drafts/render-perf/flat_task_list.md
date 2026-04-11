# Render Perf — Task Scratchpad

Flat list of known optimization opportunities. Not ordered, not grouped.

---

- **portal_vis.rs:291–293** — Replace per-hop `Vec` clone in `flood()` with push/pop backtracking on a shared `&mut Vec<usize>`
- **portal_vis.rs:319–320** — Thread scratch buffers through `clip_polygon_to_frustum` to avoid two `Vec<Vec3>` allocations per portal per frame
- **render.rs:892, 938** — `collect_visible_leaf_indices` called twice per frame when wireframe overlay is active; compute once, pass to both draw passes
- **render.rs:964–989** — `collect_visible_leaf_indices` is O(leaves × ranges); precompute a leaf → `(min_offset, max_offset)` table at load time to eliminate the inner range scan
- **render.rs:904** — Redundant `set_bind_group` calls; skip when texture index hasn't changed from previous draw call
- **render.rs (load time)** — Sort `leaf_texture_sub_ranges` entries by `texture_index` at load time to maximize bind group skip hits
- **render.rs:94–105** — `build_uniform_data` allocates an 80-byte `Vec<u8>` every frame; replace with a stack array or `bytemuck` cast
- **visibility.rs:273** — `collect_visible_faces` allocates `Vec::new()` for `DrawRange`s every frame; accept a `&mut Vec<DrawRange>` scratch parameter instead
- **visibility.rs:534–550** — Double `face_meta` iteration per visible leaf in the portal path (once to count `pvs_faces`, once to push `DrawRange`s); merge into a single loop
- **visibility.rs:467, 519, 580, 617** — Other per-frame `Vec::new()` / allocation sites in visibility paths; audit for scratch-buffer applicability
- **input/mod.rs:171–177** — `snapshot()` deduplicates action bindings via `collect::<HashSet<_>>().into_iter().collect()` every frame; precompute the unique action list at binding setup time and cache it on `InputSystem`
- **input/mod.rs:206** — `self.prev_button_states = button_states.clone()` — full HashMap clone per frame; replace with `self.prev_button_states.clear(); self.prev_button_states.extend(button_states.iter().map(|(&k, &v)| (k, v)))` to reuse existing capacity
- **main.rs:534–544** — `format!()` allocates a new String every frame for the window title diagnostic; throttle to every N frames, or use a reusable `String` with `clear()` + `write!()`
- **input/bindings.rs:104–111** — `resolve_axis_values()` allocates `Vec::new()` for a result that holds at most 2 items (displacement + velocity); replace with `[Option<AxisValue>; 2]` or a fixed small-array return type
- **visibility.rs:580** — `Vec::new()` for `DrawRange`s in the no-PVS fallback path; per-frame on levels without precomputed PVS; same scratch-buffer fix as line 273
- **visibility.rs:636–657** — PVS path counts non-zero faces (first `face_meta` pass) before the AABB-frustum cull; culled leaves pay for the count scan even though their ranges are never pushed; defer count or fuse all three operations (count + cull + push) into one loop
