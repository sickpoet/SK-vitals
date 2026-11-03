param(
  [string]$Out = "F:\AI\sickpoet\SK-vitals\docs\preview.png",
  [int]$TargetW = 768
)
$root = "F:\AI\sickpoet\SK-vitals"
$tmp = "$root\target\_prev_raw.png"
powershell -ExecutionPolicy Bypass -File "$root\tools\shot.ps1" -Out $tmp -Pad 12 | Out-Null
Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Image]::FromFile($tmp)
$scale = $TargetW / $src.Width
$oh = [int]($src.Height * $scale)
$bmp = New-Object System.Drawing.Bitmap $TargetW, $oh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.DrawImage($src, (New-Object System.Drawing.Rectangle 0,0,$TargetW,$oh), (New-Object System.Drawing.Rectangle 0,0,$src.Width,$src.Height), [System.Drawing.GraphicsUnit]::Pixel)
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
$src.Dispose()
Remove-Item $tmp -Force
Write-Output "SAVED $Out ($TargetW x $oh)"
