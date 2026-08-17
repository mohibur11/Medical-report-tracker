<#
.SYNOPSIS
  Phase 0 spike 3: measure Windows.Media.Ocr accuracy on real medical documents.

.DESCRIPTION
  Calls the OCR engine built into Windows 10/11 — the engine the app plans to ship
  with, at zero bundle cost — over a folder of scans, and reports what it actually
  reads. No Rust required: this is the same engine the `windows` crate will call,
  reached here through the WinRT projection in Windows PowerShell 5.1.

  This spike exists to answer one question before three weeks are spent on Phase 3:
  DOES THE CORRECT DATE APPEAR ANYWHERE IN THE OCR OUTPUT?

  Field extraction is a ranking problem layered on top of recognition. If the date
  is not in the text at all, no amount of ranking recovers it and the phase should
  be cut. The plan's gate is ~70% on printed reports.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File spikes\ocr.ps1 -Path "D:\scans" -Limit 30
#>
param(
  [Parameter(Mandatory = $true)][string]$Path,
  [int]$Limit = 30,
  [switch]$ShowText
)

$ErrorActionPreference = 'Stop'

# --- WinRT plumbing -------------------------------------------------------
Add-Type -AssemblyName System.Runtime.WindowsRuntime | Out-Null

[Windows.Media.Ocr.OcrEngine, Windows.Media, ContentType = WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType = WindowsRuntime] | Out-Null
[Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime] | Out-Null
[Windows.Globalization.Language, Windows.Globalization, ContentType = WindowsRuntime] | Out-Null

$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and
    $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
  })[0]

function Await($op, $resultType) {
  $task = $asTaskGeneric.MakeGenericMethod($resultType).Invoke($null, @($op))
  $task.Wait(-1) | Out-Null
  $task.Result
}

# --- engine ---------------------------------------------------------------
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $engine) {
  Write-Host 'FATAL: no OCR engine available for the current user profile languages.' -ForegroundColor Red
  Write-Host 'Install an English language pack, or fall back to the RapidOCR ONNX engine.'
  exit 1
}

Write-Host ''
Write-Host '=== OCR SPIKE (Windows.Media.Ocr) ===' -ForegroundColor Cyan
Write-Host "engine language : $($engine.RecognizerLanguage.DisplayName) [$($engine.RecognizerLanguage.LanguageTag)]"
$available = [Windows.Media.Ocr.OcrEngine]::AvailableRecognizerLanguages
Write-Host "available langs : $(($available | ForEach-Object { $_.LanguageTag }) -join ', ')"
Write-Host "max image dim   : $([Windows.Media.Ocr.OcrEngine]::MaxImageDimension) px"
Write-Host ''

# --- date patterns --------------------------------------------------------
# Deliberately broad. The question at this stage is whether a date is PRESENT in
# the recognised text, not whether the right one can be picked out.
$datePatterns = @(
  '\b\d{1,2}[/\-.]\d{1,2}[/\-.]\d{2,4}\b',
  '\b\d{4}[/\-.]\d{1,2}[/\-.]\d{1,2}\b',
  '\b\d{1,2}[\s\-]?(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*[\s\-,]?\s?\d{2,4}\b',
  '\b(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2},?\s+\d{4}\b'
)

# Labels that mark WHICH date a lab report means. Their presence is what makes
# ranking possible in Phase 3; their absence means manual entry is the honest path.
$anchorPositive = 'collect|sample|drawn|received|reported|report date|test date|registered|specimen'
$anchorNegative = 'birth|d\.?o\.?b|age|printed on|print date'

$files = Get-ChildItem -Path $Path -Recurse -File |
  Where-Object { $_.Extension -match '^\.(jpg|jpeg|png|tif|tiff|bmp)$' } |
  Select-Object -First $Limit

if ($files.Count -eq 0) {
  Write-Host "No images found under $Path" -ForegroundColor Yellow
  exit 1
}

$results = @()
$i = 0

foreach ($f in $files) {
  $i++
  Write-Progress -Activity 'OCR' -Status "$i/$($files.Count) $($f.Name)" -PercentComplete (100 * $i / $files.Count)

  try {
    $sw = [Diagnostics.Stopwatch]::StartNew()

    $sf = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($f.FullName)) ([Windows.Storage.StorageFile])
    $stream = Await ($sf.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
    $decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
    $bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
    $ocr = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])

    $sw.Stop()
    $text = $ocr.Text
    # @() is load-bearing: .Count on a WinRT IReadOnlyList projects per-element
    # in PS 5.1 and yields an array instead of a scalar.
    $lineCount = @($ocr.Lines).Count
    $wordCount = @($ocr.Lines | ForEach-Object { @($_.Words).Count } | Measure-Object -Sum).Sum

    $dates = @()
    foreach ($p in $datePatterns) {
      foreach ($m in [regex]::Matches($text, $p, 'IgnoreCase')) { $dates += $m.Value }
    }
    $dates = $dates | Select-Object -Unique

    $results += [pscustomobject]@{
      File          = $f.Name
      Px            = "$($decoder.PixelWidth)x$($decoder.PixelHeight)"
      Ms            = [int]$sw.ElapsedMilliseconds
      Lines         = $lineCount
      Words         = $wordCount
      Dates         = ($dates -join ' | ')
      DateCount     = $dates.Count
      PosAnchor     = [bool]([regex]::IsMatch($text, $anchorPositive, 'IgnoreCase'))
      NegAnchor     = [bool]([regex]::IsMatch($text, $anchorNegative, 'IgnoreCase'))
      Text          = $text
    }

    if ($ShowText) {
      Write-Host "--- $($f.Name) ---" -ForegroundColor DarkGray
      Write-Host $text
    }
  }
  catch {
    Write-Host "  FAILED $($f.Name): $($_.Exception.Message)" -ForegroundColor Yellow
    $results += [pscustomobject]@{
      File = $f.Name; Px = '?'; Ms = 0; Lines = 0; Words = 0
      Dates = ''; DateCount = 0; PosAnchor = $false; NegAnchor = $false; Text = ''
    }
  }
}
Write-Progress -Activity 'OCR' -Completed

# --- report ---------------------------------------------------------------
$n = @($results).Count
$withText = @($results | Where-Object { $_.Words -gt 20 }).Count
$withDate = @($results | Where-Object { $_.DateCount -gt 0 }).Count
$withAnchor = @($results | Where-Object { $_.PosAnchor }).Count
$ambiguous = @($results | Where-Object { $_.DateCount -gt 1 }).Count
$avgMs = [int](@($results | Measure-Object -Property Ms -Average).Average)

$results | Select-Object File, Px, Ms, Lines, Words, DateCount, PosAnchor, Dates |
  Format-Table -AutoSize | Out-String -Width 250 | Write-Host

$pct = { param($a, $b) if (-not $b) { '0.0' } else { [math]::Round(100 * [int]$a / [int]$b, 1) } }

Write-Host '--- RESULTS ---' -ForegroundColor Cyan
Write-Host "files processed            : $n"
Write-Host "recognised as text (>20 w) : $withText  ($(& $pct $withText $n)%)"
Write-Host "AT LEAST ONE DATE FOUND    : $withDate  ($(& $pct $withDate $n)%)   <-- the gate"
Write-Host "  of those, ambiguous (>1) : $ambiguous  ($(& $pct $ambiguous $withDate)% of hits) -- ranking required"
Write-Host "positive date anchor seen  : $withAnchor  ($(& $pct $withAnchor $n)%)  -- makes ranking possible"
Write-Host "mean OCR time              : $avgMs ms/page"
Write-Host ''

$rate = [double](& $pct $withDate $n)
Write-Host '--- GATE (plan: cut Phase 3 below ~70% on printed reports) ---' -ForegroundColor Cyan
if ($rate -ge 70) {
  Write-Host "PASS - $rate% date recall. OCR prefill is worth building." -ForegroundColor Green
}
elseif ($rate -ge 40) {
  Write-Host "MARGINAL - $rate% date recall." -ForegroundColor Yellow
  Write-Host 'Try image conditioning (deskew, quad-detect, perspective warp) before deciding;'
  Write-Host 'the plan expects that to buy more than any recognizer upgrade.'
}
else {
  Write-Host "FAIL - $rate% date recall. Cut Phase 3; manual entry is the honest path." -ForegroundColor Red
}

$out = Join-Path (Split-Path $PSScriptRoot -Parent) 'spikes\out\ocr-result.json'
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
# WriteAllText with an explicit no-BOM encoding: PowerShell 5.1's -Encoding utf8
# emits a byte-order mark, and a BOM makes the file invalid JSON to strict parsers.
[System.IO.File]::WriteAllText($out, ($results | ConvertTo-Json -Depth 4), (New-Object System.Text.UTF8Encoding($false)))
Write-Host ""
Write-Host "wrote $out"
