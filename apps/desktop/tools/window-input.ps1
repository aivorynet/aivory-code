# Send mouse input to the app window for screenshot checks. The messages go straight to the
# window (PostMessage), so other windows on top do not matter and the real cursor stays put.
# Coordinates are logical client pixels (as in the artboards); the script applies the DPI scale.
# Examples:
#   window-input.ps1 -Action move -X 400 -Y 700
#   window-input.ps1 -Action click -X 46 -Y 170
#   window-input.ps1 -Action wheel -X 700 -Y 600 -Delta -5   (negative = scroll down)
#   window-input.ps1 -Action type -Text "hello"   then   -Action enter
param(
    [string]$ProcessName = "aivory",
    [Parameter(Mandatory)][ValidateSet("move", "click", "wheel", "type", "enter")][string]$Action,
    [string]$Text = "",
    [int]$X = 0,
    [int]$Y = 0,
    [int]$Delta = -3
)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Inp {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
}
"@
[Inp]::SetProcessDPIAware() | Out-Null
$p = Get-Process -Name $ProcessName -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
$h = $p.MainWindowHandle
$scale = [Inp]::GetDpiForWindow($h) / 96.0
$cx = [int]($X * $scale); $cy = [int]($Y * $scale)
$client = [IntPtr](($cy -shl 16) -bor ($cx -band 0xFFFF))
function Post($msg, $w, $l) { [Inp]::PostMessage($h, $msg, [IntPtr]$w, $l) | Out-Null }
Post 0x0200 0 $client
Start-Sleep -Milliseconds 100
switch ($Action) {
    "click" {
        Post 0x0201 1 $client
        Start-Sleep -Milliseconds 60
        Post 0x0202 0 $client
    }
    "type" {
        foreach ($ch in $Text.ToCharArray()) { Post 0x0102 ([int]$ch) ([IntPtr]1); Start-Sleep -Milliseconds 15 }
    }
    "enter" {
        Post 0x0100 0x0D ([IntPtr]0x001C0001)
        Post 0x0102 0x0D ([IntPtr]0x001C0001)
        Post 0x0101 0x0D ([IntPtr]0xC01C0001)
    }
    "wheel" {
        $pt = New-Object Inp+POINT
        $pt.X = $cx; $pt.Y = $cy
        [Inp]::ClientToScreen($h, [ref]$pt) | Out-Null
        $screen = [IntPtr](($pt.Y -shl 16) -bor ($pt.X -band 0xFFFF))
        $step = [Math]::Sign($Delta) * 120
        for ($i = 0; $i -lt [Math]::Abs($Delta); $i++) {
            Post 0x020A (($step -band 0xFFFF) -shl 16) $screen
            Start-Sleep -Milliseconds 30
        }
    }
}
Start-Sleep -Milliseconds 400
