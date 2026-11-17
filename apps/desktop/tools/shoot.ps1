# Screenshot every screen of every look of the AIVory desktop app. No mouse, no keys, no focus.
# Usage: shoot.ps1 -Exe <copy of aivory.exe> -Out <dir> [-Looks a,b] [-Screens x,y]
param(
  [Parameter(Mandatory)][string]$Exe,
  [Parameter(Mandatory)][string]$Out,
  [string[]]$Looks = @('minimal','bridge','operator','neon','navigator','holo','tactical','command'),
  [string[]]$Screens = @(),
  [string]$Fit = '',   # force the text fit, for example 1.4 (a 4K window at 100 %)
  [string]$Size = ''   # logical window size, for example 2560x1440; the PNG keeps the real client size
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
$Looks = @($Looks | ForEach-Object { $_ -split ',' }); $Screens = @($Screens | ForEach-Object { $_ -split ',' } | Where-Object { $_ })

Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System; using System.Collections.Generic; using System.Drawing; using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class Shot {
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
  [StructLayout(LayoutKind.Sequential)] struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] struct POINT { public int X, Y; }
  public static void Init() { SetProcessDPIAware(); }
  public static IntPtr FindWindow(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) { RECT r; GetClientRect(h, out r); if (r.R > 300 && r.B > 300) { found = h; return false; } }
      return true; }, IntPtr.Zero);
    return found;
  }
  // HWND_BOTTOM, SWP_NOSIZE|NOMOVE|NOACTIVATE. Never activates, never moves.
  public static void ToBottom(IntPtr h) { SetWindowPos(h, (IntPtr)1, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010); }
  // Returns the client area scaled to 1440x900, or null when the window has no size.
  public static Bitmap Capture(IntPtr h, bool native) {
    RECT w, c; GetWindowRect(h, out w); GetClientRect(h, out c);
    int ww = w.R - w.L, wh = w.B - w.T, cw = c.R, ch = c.B;
    if (ww < 100 || wh < 100 || cw < 100 || ch < 100) return null;
    POINT o = new POINT(); ClientToScreen(h, ref o);
    int ox = o.X - w.L, oy = o.Y - w.T;
    using (Bitmap full = new Bitmap(ww, wh, PixelFormat.Format32bppArgb)) {
      using (Graphics g = Graphics.FromImage(full)) {
        IntPtr dc = g.GetHdc();
        try { PrintWindow(h, dc, 2); } finally { g.ReleaseHdc(dc); }
      }
      int tw = native ? cw : 1440, th = native ? ch : 900;
      Bitmap res = new Bitmap(tw, th, PixelFormat.Format24bppRgb);
      using (Graphics g = Graphics.FromImage(res)) {
        g.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBicubic;
        g.DrawImage(full, new Rectangle(0, 0, tw, th), new Rectangle(ox, oy, cw, ch), GraphicsUnit.Pixel);
      }
      return res;
    }
  }
  // Distinct colours among a 48x30 sample grid. A blank frame has 1 or 2.
  public static int Colors(Bitmap b) {
    HashSet<int> s = new HashSet<int>();
    for (int y = 0; y < 30; y++) for (int x = 0; x < 48; x++) s.Add(b.GetPixel(x * 30 + 7, y * 30 + 7).ToArgb() & 0xF8F8F8);
    return s.Count;
  }
}
'@
Add-Type -AssemblyName System.Drawing
[Shot]::Init()

# screen name -> env values (all start with --demo --demo-rich --offscreen)
$all = [ordered]@{
  'map'      = @{ AIVORY_SCREEN = 'warroom' }                                  # the war room, nothing selected
  'warroom'  = @{ AIVORY_SELECT = '5e81' }                                     # a selected working session (Bridge war room)
  'site'     = @{ AIVORY_SITE = '5e81' }                                       # the site of that session (Bridge war room)
  'session'  = @{ AIVORY_SESSION = '7f3a' }                                   # waits for a permission decision (task t8)
  'focus'    = @{ AIVORY_FOCUS = '5e81'; AIVORY_FOCUS_VIEW = 'split' }        # working session, split view
  'inbox'    = @{ AIVORY_SCREEN = 'inbox' }
  'tasks'    = @{ AIVORY_SCREEN = 'tasks' }
  'repos'    = @{ AIVORY_SCREEN = 'repos' }
  'settings' = @{ AIVORY_SCREEN = 'settings' }
  'newsession' = @{ AIVORY_LAUNCH = 'session' }
  'organizer' = @{ AIVORY_LAUNCH = 'organizer' }
}
# 'lens' has no env hook (Screen::Lens opens from a Repos file click only): not covered.
$names = if ($Screens.Count) { $Screens } else { @($all.Keys) }
$envKeys = 'AIVORY_LOOK','AIVORY_SITE','AIVORY_SESSION','AIVORY_FOCUS','AIVORY_FOCUS_VIEW','AIVORY_SCREEN','AIVORY_LAUNCH','AIVORY_SETTINGS','AIVORY_SELECT'
$waits = 3, 5, 8   # seconds after the window is found: three different waits
$failed = @()

function Shoot($look, $name, $png) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $Exe; $psi.Arguments = '--demo --demo-rich --offscreen'
  $psi.UseShellExecute = $false; $psi.WorkingDirectory = Split-Path $Exe
  foreach ($k in $envKeys) { $psi.EnvironmentVariables.Remove($k) }
  $psi.EnvironmentVariables['AIVORY_LOOK'] = $look
  $psi.EnvironmentVariables.Remove('AIVORY_FIT'); if ($Fit) { $psi.EnvironmentVariables['AIVORY_FIT'] = $Fit }
  $psi.EnvironmentVariables.Remove('AIVORY_WINDOW'); if ($Size) { $psi.EnvironmentVariables['AIVORY_WINDOW'] = $Size }
  foreach ($kv in $all[$name].GetEnumerator()) { $psi.EnvironmentVariables[$kv.Key] = $kv.Value }
  $p = [System.Diagnostics.Process]::Start($psi)
  try {
    $h = [IntPtr]::Zero
    for ($i = 0; $i -lt 60 -and $h -eq [IntPtr]::Zero; $i++) {
      if ($p.HasExited) { return $false }
      Start-Sleep -Milliseconds 250; $h = [Shot]::FindWindow([uint32]$p.Id)
    }
    if ($h -eq [IntPtr]::Zero) { return $false }
    [Shot]::ToBottom($h)
    $ok = $false
    foreach ($w in $(if ($all[$name].ContainsKey("AIVORY_SELECT") -or $all[$name].ContainsKey("AIVORY_SITE")) { 11, 16 } else { $waits })) {
      Start-Sleep -Seconds $w
      [Shot]::ToBottom($h)
      $bmp = [Shot]::Capture($h, [bool]$Size)
      if ($bmp -and [Shot]::Colors($bmp) -ge 12) { $bmp.Save($png, [System.Drawing.Imaging.ImageFormat]::Png); $ok = $true; $bmp.Dispose(); break }
      if ($bmp) { $last = $bmp }
    }
    if (-not $ok -and $last) { $last.Save($png, [System.Drawing.Imaging.ImageFormat]::Png); $last.Dispose() }
    return $ok
  } finally {
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
  }
}

function Sheet($look) {
  $enc = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object MimeType -eq 'image/jpeg'
  $ep = New-Object System.Drawing.Imaging.EncoderParameters 1
  $ep.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter ([System.Drawing.Imaging.Encoder]::Quality), 80L
  $files = $names | ForEach-Object { @{ n = $_; f = Join-Path $Out "$look-$_.png" } } | Where-Object { Test-Path $_.f }
  if (-not $files) { return }
  $cols = 2; $rows = [math]::Ceiling($files.Count / $cols); $band = 30
  $sheet = New-Object System.Drawing.Bitmap ($cols * 720), ($rows * (450 + $band))
  $g = [System.Drawing.Graphics]::FromImage($sheet)
  $g.Clear([System.Drawing.Color]::FromArgb(40, 40, 40)); $g.InterpolationMode = 'HighQualityBicubic'
  $font = New-Object System.Drawing.Font 'Segoe UI', 13, ([System.Drawing.FontStyle]::Bold)
  $i = 0
  foreach ($e in $files) {
    $img = [System.Drawing.Image]::FromFile($e.f)
    $x = ($i % $cols) * 720; $y = [math]::Floor($i / $cols) * (450 + $band)
    $g.DrawImage($img, $x, $y + $band, 720, 450); $img.Dispose()
    $label = "$look - $($e.n)"
    $g.DrawString($label, $font, [System.Drawing.Brushes]::White, $x + 8, $y + 2)
    $i++
  }
  $g.Dispose()
  $sheet.Save((Join-Path $Out "sheet-$look.jpg"), $enc, $ep); $sheet.Dispose()
}

$sw = [Diagnostics.Stopwatch]::StartNew()
foreach ($look in $Looks) {
  foreach ($name in $names) {
    $png = Join-Path $Out "$look-$name.png"
    $ok = Shoot $look $name $png
    if (-not $ok) { $failed += "$look-$name"; Write-Host "BLANK/FAILED $look-$name" } else { Write-Host "ok $look-$name" }
  }
  Sheet $look
}
Write-Host ("done in {0:n0} s; failed: {1}" -f $sw.Elapsed.TotalSeconds, ($failed -join ', '))
