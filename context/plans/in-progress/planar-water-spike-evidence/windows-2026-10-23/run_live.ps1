# Run from the spike workspace root after every capture/build/bake has stopped.
$ErrorActionPreference='Stop'
$liveRoot=Join-Path $PSScriptRoot 'live'
if(Test-Path $liveRoot){throw 'Use a fresh live output directory.'}
New-Item -ItemType Directory $liveRoot | Out-Null
$env:WGPU_BACKEND='vulkan'
$env:WGPU_VALIDATION_INDIRECT_CALL='1'
$env:POSTRETRO_CPU_TIMING='1'
$env:POSTRETRO_GPU_TIMING='1'
$env:RUST_LOG='info'
Remove-Item Env:POSTRETRO_SH_STREAMING -ErrorAction SilentlyContinue
$binary=(Resolve-Path target/debug/postretro.exe).Path
# Pawn origin is the capsule centre: half-height 0.8 + radius 0.2.
# Start clear of the deck; gravity settles it to centre y=1, eye y=1.5.
$launchArgs=@('--mod','dev','maps/reflection-lake-test.prl','--core-root','core','--windowed','--start-pose=-16.8656,1.2,-48.768,90,-10','--pool-seed','1')
$records=@()
$run=0
foreach($mode in @('ssr','planar','planar','ssr')){
 $run++
 $env:POSTRETRO_PLANAR_REFLECTION=if($mode -eq 'planar'){'1'}else{'0'}
 $key="$mode-run$run"
 $started=(Get-Date).ToUniversalTime().ToString('o')
 # Visible window is required: the engine suspends rendering while hidden.
 $child=Start-Process -FilePath $binary -ArgumentList $launchArgs -WindowStyle Normal -PassThru -RedirectStandardOutput "$liveRoot/$key.stdout.log" -RedirectStandardError "$liveRoot/$key.stderr.log"
 try {
  $exited=$child.WaitForExit(20000)
  $records+=@{key=$key;pid=$child.Id;started_utc=$started;seconds=20;exited_early=$exited;arguments=$launchArgs;backend='vulkan';indirect_validation='on';sh_streaming='normal';planar=$env:POSTRETRO_PLANAR_REFLECTION;reflection_quality='saved player setting; verify log';start_pose='pawn origin, not capture eye'}
 } finally {
  if(-not $child.HasExited){Stop-Process -Id $child.Id}
 }
 $records | ConvertTo-Json -Depth 5 | Set-Content "$liveRoot/manifest.json"
 Write-Output "$key finished; inspect CPU windows, reveal and actual extent before accepting it."
}
