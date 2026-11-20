# playback_trace fixture

A slice of one real camera-route playback run with an attached Metal System Trace, for `test_playback_trace_reduce.py`.

- **Source:** the camera-path-playback Mac checkpoint, branch `mac/camera-path-checkpoint` at 159f47d64 (`context/plans/in-progress/camera-path-playback/mac-results/`). The run used campaign-test, a 40-step far-point route (1,746 poses), a 900-frame hold, vsync and the Task 4 build, so the record has no drain, cold or growth columns.
- **Rows kept:** record rows and the matching `metal-application-intervals` and `metal-gpu-intervals` rows for three frame ranges around frames 1136–1139 (SH upload frames) and 1248 (a lightmap drain), plus hold rows before the trace attached. There are also 8 `ioreg` polls and the export's TOC summary.
- **Regenerating:** cut the same table slices from a newer full-record run, then re-derive the hand-checked numbers in `SampleTests`.
