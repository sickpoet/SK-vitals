param(
  [string]$Out = "F:\AI\sickpoet\SK-vitals\target\shot.png",
  [int]$Pad = 40
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern IntPtr FindWindow(string c, string n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@
$h = [W]::FindWindow("SKVitalsPanel", [NullString]::Value)
if ($h -eq [IntPtr]::Zero) { Write-Output "NOT FOUND"; exit 1 }
$r = New-Object W+RECT
[void][W]::GetWindowRect($h, [ref]$r)
$x = $r.L - $Pad; $y = $r.T - $Pad
$w = ($r.R - $r.L) + $Pad * 2; $ht = ($r.B - $r.T) + $Pad * 2
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "SAVED $Out ($w x $ht) at ($x,$y)"
