# Photographs the real window of a build: starts `annulus-app --preview` (a throwaway demo
# vault, never anyone's data), captures the window with PrintWindow and closes it.
#   scripts\ci\capture-window.ps1 -Bin target\release\annulus-app.exe -Out shots
# Needs a logged-in desktop session. Writes window-dark.png and window-light.png.
param([string]$Bin = "target\release\annulus-app.exe", [string]$Out = "shots")
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Win {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
[Win]::SetProcessDPIAware() | Out-Null
foreach ($theme in @("dark", "light")) {
  $args = @("--preview")
  if ($theme -eq "light") { $args += "--light" }
  $p = Start-Process -FilePath $Bin -ArgumentList $args -PassThru
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 50 -and $h -eq [IntPtr]::Zero; $i++) {
    Start-Sleep -Milliseconds 200
    $p.Refresh()
    $h = $p.MainWindowHandle
  }
  if ($h -eq [IntPtr]::Zero) { Stop-Process $p -Force; throw "no window" }
  Start-Sleep -Milliseconds 800
  $r = New-Object Win+RECT
  [Win]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.Right - $r.Left; $hgt = $r.Bottom - $r.Top
  $bmp = New-Object System.Drawing.Bitmap $w, $hgt
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $dc = $g.GetHdc()
  # PW_RENDERFULLCONTENT (2): content drawn by Direct2D/DirectComposition too.
  [Win]::PrintWindow($h, $dc, 2) | Out-Null
  $g.ReleaseHdc($dc); $g.Dispose()
  $bmp.Save((Join-Path $Out "window-$theme.png"), [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  Stop-Process $p -Force
}
