param(
  [string]$Out = "F:\AI\sickpoet\SK-vitals\docs\preview.png",
  [int]$TargetW = 768
)
$root = "F:\AI\sickpoet\SK-vitals"
$tmp = "$root\target\_prev_raw.png"
# Pad 0：拿到与窗口同尺寸的原始图，后面自己按卡片位置裁
powershell -ExecutionPolicy Bypass -File "$root\tools\shot.ps1" -Out $tmp -Pad 0 | Out-Null

Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Image]::FromFile($tmp)

# 窗口尺寸 = 卡片 + 四周留白（与 src/ui.rs 里的 MARGIN_* 一致）
$winW = 344.0; $winH = 76.0
$scale = $src.Width / $winW
$marginX = 12.0 * $scale
$marginTop = 10.0 * $scale
$cardW = 320.0 * $scale
$cardH = 50.0 * $scale

$crop = New-Object System.Drawing.Rectangle
$crop.X = [int][math]::Round($marginX)
$crop.Y = [int][math]::Round($marginTop)
$crop.Width = [int][math]::Round($cardW)
$crop.Height = [int][math]::Round($cardH)

$outScale = $TargetW / $crop.Width
$oh = [int][math]::Round($crop.Height * $outScale)
$bmp = New-Object System.Drawing.Bitmap $TargetW, $oh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.DrawImage($src, (New-Object System.Drawing.Rectangle 0,0,$TargetW,$oh), $crop, [System.Drawing.GraphicsUnit]::Pixel)
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
$src.Dispose()
Remove-Item $tmp -Force
Write-Output "SAVED $Out ($TargetW x $oh)"
