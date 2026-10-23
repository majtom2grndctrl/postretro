param([int]$Seconds = 24, [string]$Out)
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class Z {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetTopWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
  public static string Cls(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
  public static string Txt(IntPtr h) { var s = new StringBuilder(256); GetWindowText(h, s, 256); return s.ToString(); }
  public static string Above(IntPtr target) {
    RECT t; GetWindowRect(target, out t); var sb = new StringBuilder();
    for (IntPtr h = GetTopWindow(IntPtr.Zero); h != IntPtr.Zero && h != target; h = GetWindow(h, 2)) {
      if (!IsWindowVisible(h) || IsIconic(h)) continue;
      int cloaked; DwmGetWindowAttribute(h, 14, out cloaked, 4); if (cloaked != 0) continue;
      RECT r; GetWindowRect(h, out r);
      int w = Math.Min(r.R, t.R) - Math.Max(r.L, t.L), hh = Math.Min(r.B, t.B) - Math.Max(r.T, t.T);
      if (w > 0 && hh > 0) { uint pid; GetWindowThreadProcessId(h, out pid); sb.AppendFormat("[{0} '{1}' pid {2} overlap {3}x{4}] ", Cls(h), Txt(h), pid, w, hh); }
    }
    return sb.Length == 0 ? "none" : sb.ToString();
  }
}
"@
$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
  $fg = [Z]::GetForegroundWindow(); $fgpid = 0; [void][Z]::GetWindowThreadProcessId($fg, [ref]$fgpid)
  $eng = Get-Process postretro -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($eng -and $eng.MainWindowHandle -ne 0) {
    $h = $eng.MainWindowHandle; $r = New-Object Z+RECT; [void][Z]::GetWindowRect($h, [ref]$r)
    $line = "{0:HH:mm:ss} fg={1} class={2} '{3}' | engine pid={4} rect={5},{6},{7},{8} minimized={9} overlapped_above_by={10}" -f (Get-Date), $fgpid, [Z]::Cls($fg), [Z]::Txt($fg), $eng.Id, $r.L, $r.T, $r.R, $r.B, [Z]::IsIconic($h), [Z]::Above($h)
  } else { $line = "{0:HH:mm:ss} no engine window" -f (Get-Date) }
  Add-Content -Path $Out -Value $line
  Start-Sleep -Seconds 2
}
