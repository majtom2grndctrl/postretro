param([int]$Seconds = 24, [string]$Out)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Fg {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
}
"@
$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
  $h = [Fg]::GetForegroundWindow(); $fgpid = 0; [void][Fg]::GetWindowThreadProcessId($h, [ref]$fgpid)
  $fgname = (Get-Process -Id $fgpid -ErrorAction SilentlyContinue).ProcessName
  $eng = Get-Process postretro -ErrorAction SilentlyContinue | Select-Object -First 1
  $iconic = if ($eng -and $eng.MainWindowHandle -ne 0) { [Fg]::IsIconic($eng.MainWindowHandle) } else { $null }
  $line = "{0:HH:mm:ss} foreground={1}({2}) engine_pid={3} engine_hwnd={4} engine_minimized={5}" -f (Get-Date), $fgname, $fgpid, $eng.Id, $eng.MainWindowHandle, $iconic
  Add-Content -Path $Out -Value $line
  Start-Sleep -Seconds 2
}
