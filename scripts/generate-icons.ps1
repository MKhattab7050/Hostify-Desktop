# Offline renderer for our original SVG's rounded rectangle and line/circle geometry.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$icons = Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri/icons'
New-Item -ItemType Directory -Force -Path $icons | Out-Null
$svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="15" fill="#305b49"/><g transform="translate(8 8) scale(2)" fill="none" stroke="#e9f5ce" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 18V8l8 5 8-5v10"/><circle cx="12" cy="6" r="1.6" fill="#e9f5ce" stroke="none"/></g></svg>'
Set-Content -LiteralPath (Join-Path $icons 'logo.svg') -Value $svg -Encoding UTF8
$pngs = @{}
foreach ($size in @(32,48,128,256)) {
    $bitmap = New-Object System.Drawing.Bitmap($size,$size)
    $g = [System.Drawing.Graphics]::FromImage($bitmap)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.ScaleTransform($size/64.0,$size/64.0)
    $shape = New-Object System.Drawing.Drawing2D.GraphicsPath
    foreach ($corner in @(@(0,0,180),@(34,0,270),@(34,34,0),@(0,34,90))) { $shape.AddArc($corner[0],$corner[1],30,30,$corner[2],90) }
    $shape.CloseFigure()
    $bg = New-Object System.Drawing.SolidBrush([System.Drawing.ColorTranslator]::FromHtml('#305b49'))
    $fg = New-Object System.Drawing.SolidBrush([System.Drawing.ColorTranslator]::FromHtml('#e9f5ce'))
    $pen = New-Object System.Drawing.Pen($fg,4.8)
    $pen.StartCap = $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $g.FillPath($bg,$shape)
    $points = [System.Drawing.PointF[]]@((New-Object System.Drawing.PointF(16,44)),(New-Object System.Drawing.PointF(16,24)),(New-Object System.Drawing.PointF(32,34)),(New-Object System.Drawing.PointF(48,24)),(New-Object System.Drawing.PointF(48,44)))
    $g.DrawLines($pen,$points)
    $g.FillEllipse($fg,28.8,16.8,6.4,6.4)
    $stream = New-Object System.IO.MemoryStream
    $bitmap.Save($stream,[System.Drawing.Imaging.ImageFormat]::Png)
    $pngs[$size] = $stream.ToArray()
    $filename = switch ($size) {32 {'32x32.png'};48 {'48x48.png'};128 {'128x128.png'};256 {'128x128@2x.png'}}
    [System.IO.File]::WriteAllBytes((Join-Path $icons $filename),$pngs[$size])
    $stream.Dispose(); $pen.Dispose(); $fg.Dispose(); $bg.Dispose(); $shape.Dispose(); $g.Dispose(); $bitmap.Dispose()
}
# ICO directory with PNG payloads at four resolutions.
$stream = [System.IO.File]::Create((Join-Path $icons 'icon.ico'))
$writer = New-Object System.IO.BinaryWriter($stream)
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]4)
    $offset = 6 + 16*4
    foreach ($size in @(32,48,128,256)) {
        $dimension = if ($size -eq 256) {0} else {$size}
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension); $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32); $writer.Write([uint32]$pngs[$size].Length); $writer.Write([uint32]$offset)
        $offset += $pngs[$size].Length
    }
    foreach ($size in @(32,48,128,256)) { $writer.Write([byte[]]$pngs[$size]) }
} finally { $writer.Dispose(); $stream.Dispose() }
