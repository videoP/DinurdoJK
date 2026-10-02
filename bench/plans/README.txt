Bench plans
===========

Run:      bench.cmd bench\plans\deluxe.bench
Options:  --launches N   override launches        --only a,b   run only those cases
          --dry-run      print launch commands only
Output:   bench\results\<plan>-<timestamp>.txt  (summary report; give this to Claude)
          bench\results\<plan>-<timestamp>-logs\ (full latest.log per launch)

Needs a built target\release\DinurdoJK.exe (build.ps1 -Fast). The exe is copied to
a scratch folder, so it is fine to have the game open, but do not touch the mouse
or keyboard while a bench runs (the view drifts and the report flags it).
Your DinurdoJK.cfg is backed up and restored by scripts\bench-map.sh.

Plan file keywords (one per line, # starts a comment)
-----------------------------------------------------
map NAME                      map to load (a case can override with map=NAME)
launches N                    launches per case (default 3). Single runs vary +/-15%
settle SECONDS                time at each spot before measuring (default 3)
sample SECONDS                measured time per spot (default 5)
timeout SECONDS               per-launch limit (default 400)
set CVAR VALUE                console `set` after the map loads, for every case
cfg CVAR VALUE                cfg-file setting for every case (restart-latched cvars,
                              r_backend, r_reflectionQuality, ...). r_fullscreen 0 is
                              the default
cmd COMMAND [ARGS]            extra console command at each spot (e.g. cmd screenshot)
spot NAME X Y Z YAW [PITCH]   a place to stand: setviewpos values (z = viewpos eye z - 37).
                              Every launch visits all spots in order
case NAME TOKEN ...           one configuration. Tokens:
                                cvar=value      console set
                                cfg:cvar=value  cfg-file setting
                                cmd:name,arg    extra console command (comma-separated)
                                map=NAME        different map for this case
                              The first case is the baseline for the dms column.

Reading the report
------------------
fps / ms-per-frame   mean over launches. ms/frame is the honest unit.
dms                  ms/frame minus the first case. Positive = slower.
noise%               (max-min)/mean of per-launch FPS. A dms smaller than the noise
                     is not a real difference; add launches.
gpu_ms/per-pass      only with cfg r_gpuTimings 1
draws                world draws encoded per frame
variant              shader variant key the world pass used (shows which features
                     were compiled in)
