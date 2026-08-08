[CmdletBinding()]
param(
  [string]$Application = (Join-Path $PSScriptRoot '..\src-tauri\target\release\web-social-digest.exe'),
  [string]$TauriDriver = (Join-Path $PSScriptRoot '..\src-tauri\target\tools\tauri-driver\bin\tauri-driver.exe'),
  [string]$NativeDriver = (Join-Path $PSScriptRoot '..\src-tauri\target\tools\msedgedriver\msedgedriver.exe'),
  [string]$ScreenshotPath = '',
  [string]$Theme = '',
  [int]$WindowWidth = 0,
  [int]$WindowHeight = 0,
  [double]$ReadingScale = 0,
  [string]$ReadingScaleScreenshotPath = '',
  [int]$ZoomInSteps = 0,
  [int]$Port = 4454,
  [int]$NativePort = 4455
)

$ErrorActionPreference = 'Stop'

function Get-ProcessIdsForPath([string]$Path, [string]$Name) {
  return @(
    Get-CimInstance Win32_Process -Filter "Name = '$Name'" |
      Where-Object { $_.ExecutablePath -eq $Path } |
      ForEach-Object { [int]$_.ProcessId }
  )
}

function Wait-ForWebDriver([int]$DriverPort) {
  for ($attempt = 0; $attempt -lt 30; $attempt++) {
    try {
      $status = Invoke-RestMethod -Uri "http://127.0.0.1:$DriverPort/status" -TimeoutSec 2
      if ($status.value.ready) { return }
    } catch {
      Start-Sleep -Milliseconds 250
    }
  }
  throw 'Tauri WebDriver did not become ready.'
}

$applicationPath = (Resolve-Path $Application).Path
$tauriDriverPath = (Resolve-Path $TauriDriver).Path
$nativeDriverPath = (Resolve-Path $NativeDriver).Path
if (($WindowWidth -gt 0) -ne ($WindowHeight -gt 0)) {
  throw 'WindowWidth and WindowHeight must be supplied together.'
}
if ($Theme -and $Theme -notin @('auto', 'light', 'dark')) {
  throw 'Theme must be auto, light, or dark when supplied.'
}
if ($ReadingScale -and $ReadingScale -notin @(1, 1.25, 1.5, 2)) {
  throw 'ReadingScale must be 1, 1.25, 1.5, or 2 when supplied.'
}
if ($ZoomInSteps -lt 0 -or $ZoomInSteps -gt 5) {
  throw 'ZoomInSteps must be between 0 and 5.'
}
$toolRoot = Split-Path (Split-Path $tauriDriverPath -Parent) -Parent
$logRoot = Join-Path $toolRoot 'webdriver-logs'
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$screenshotOutputPath = if ($ScreenshotPath) { [IO.Path]::GetFullPath($ScreenshotPath, $PWD) } else { $null }
$readingScaleScreenshotOutputPath = if ($ReadingScaleScreenshotPath) {
  [IO.Path]::GetFullPath($ReadingScaleScreenshotPath, $PWD)
} else {
  $null
}

$priorAppPids = Get-ProcessIdsForPath $applicationPath 'web-social-digest.exe'
$priorNativeDriverPids = Get-ProcessIdsForPath $nativeDriverPath 'msedgedriver.exe'
$driver = $null
$sessionId = $null

try {
  $driver = Start-Process -FilePath $tauriDriverPath `
    -ArgumentList @('--port', $Port, '--native-port', $NativePort, '--native-driver', $nativeDriverPath) `
    -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput (Join-Path $logRoot 'tauri-driver.out.log') `
    -RedirectStandardError (Join-Path $logRoot 'tauri-driver.err.log')
  Wait-ForWebDriver $Port

  $sessionPayload = @{
    capabilities = @{
      alwaysMatch = @{
        'tauri:options' = @{ application = $applicationPath; args = @() }
      }
    }
  } | ConvertTo-Json -Depth 8 -Compress
  $session = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session" `
    -ContentType 'application/json' -Body $sessionPayload -TimeoutSec 30
  $sessionId = $session.value.sessionId
  if (-not $sessionId) { throw 'WebDriver returned no session id.' }
  if ($WindowWidth -gt 0) {
    $windowRect = @{ width = $WindowWidth; height = $WindowHeight } | ConvertTo-Json -Compress
    Invoke-RestMethod -Method Put -Uri "http://127.0.0.1:$Port/session/$sessionId/window/rect" `
      -ContentType 'application/json' -Body $windowRect -TimeoutSec 10 | Out-Null
  }

  $rendered = $null
  for ($attempt = 0; $attempt -lt 40; $attempt++) {
    try {
      $body = @{ script = 'return { heading: document.querySelector("h1")?.innerText, shortcut: Array.from(document.querySelectorAll("button")).find((button) => button.innerText.includes("Search & commands"))?.innerText, theme: document.documentElement.dataset.theme, viewportWidth: window.innerWidth };'; args = @() } |
        ConvertTo-Json -Compress
      $rendered = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
        -ContentType 'application/json' -Body $body -TimeoutSec 2).value
      if ($rendered.heading -and $rendered.shortcut) { break }
      Start-Sleep -Milliseconds 250
    } catch {
      Start-Sleep -Milliseconds 250
    }
  }
  if (-not $rendered.heading) { throw 'Native WebView did not expose a rendered heading.' }
  if ($rendered.shortcut -notmatch 'Ctrl K') {
    throw "Windows WebView reported an unexpected command shortcut: $($rendered.shortcut)"
  }
  if ($Theme) {
    $setTheme = @{ script = 'const input = document.querySelector(`input[value="${arguments[0]}"]`); if (!input) return false; input.click(); return true;'; args = @($Theme) } |
      ConvertTo-Json -Depth 4 -Compress
    $changedTheme = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
      -ContentType 'application/json' -Body $setTheme -TimeoutSec 10).value
    if (-not $changedTheme) { throw "The $Theme theme control was not reachable." }
    Start-Sleep -Milliseconds 100
    $activeTheme = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
      -ContentType 'application/json' -Body '{"script":"return document.documentElement.dataset.theme","args":[]}' -TimeoutSec 10).value
    if ($activeTheme -ne $Theme) { throw "The $Theme theme did not apply in the native WebView." }
    $rendered.theme = $activeTheme
  }
  if ($ZoomInSteps -gt 0) {
    $zoomIn = @{
      actions = @(@{
        type = 'key'
        id = 'keyboard'
        actions = @(
          @{ type = 'keyDown'; value = "`u{E009}" }, @{ type = 'keyDown'; value = '=' },
          @{ type = 'keyUp'; value = '=' }, @{ type = 'keyUp'; value = "`u{E009}" }
        )
      })
    } | ConvertTo-Json -Depth 8 -Compress
    for ($step = 0; $step -lt $ZoomInSteps; $step++) {
      Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/actions" `
        -ContentType 'application/json' -Body $zoomIn -TimeoutSec 10 | Out-Null
    }
    Start-Sleep -Milliseconds 250
    $zoomMetrics = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
      -ContentType 'application/json' -Body '{"script":"return { viewportWidth: window.innerWidth, devicePixelRatio: window.devicePixelRatio, visualViewportScale: window.visualViewport?.scale ?? null };","args":[]}' -TimeoutSec 10).value
    $rendered | Add-Member -NotePropertyName zoomMetrics -NotePropertyValue $zoomMetrics -Force
  }
  if ($screenshotOutputPath) {
    New-Item -ItemType Directory -Force -Path (Split-Path $screenshotOutputPath -Parent) | Out-Null
    $screenshot = (Invoke-RestMethod -Method Get -Uri "http://127.0.0.1:$Port/session/$sessionId/screenshot" -TimeoutSec 10).value
    [IO.File]::WriteAllBytes($screenshotOutputPath, [Convert]::FromBase64String($screenshot))
  }

  $controlK = @{
    actions = @(@{
      type = 'key'
      id = 'keyboard'
      actions = @(
        @{ type = 'keyDown'; value = "`u{E009}" }, @{ type = 'keyDown'; value = 'k' },
        @{ type = 'keyUp'; value = 'k' }, @{ type = 'keyUp'; value = "`u{E009}" }
      )
    })
  } | ConvertTo-Json -Depth 8 -Compress
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/actions" `
    -ContentType 'application/json' -Body $controlK -TimeoutSec 10 | Out-Null

  $dialog = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element" `
    -ContentType 'application/json' -Body '{"using":"css selector","value":"[role=dialog]"}' -TimeoutSec 10
  $dialogId = $dialog.value.'element-6066-11e4-a52e-4f735466cecf'
  $dialogText = (Invoke-RestMethod -Method Get -Uri "http://127.0.0.1:$Port/session/$sessionId/element/$dialogId/text" -TimeoutSec 10).value
  if ($dialogText -notmatch 'Command palette') { throw 'Ctrl+K did not open the command palette.' }

  $escape = @{ actions = @(@{ type = 'key'; id = 'keyboard'; actions = @(@{ type = 'keyDown'; value = "`u{E00C}" }, @{ type = 'keyUp'; value = "`u{E00C}" }) }) } |
    ConvertTo-Json -Depth 8 -Compress
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/actions" `
    -ContentType 'application/json' -Body $escape -TimeoutSec 10 | Out-Null
  Start-Sleep -Milliseconds 250
  $dialogCount = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
    -ContentType 'application/json' -Body '{"script":"return document.querySelectorAll(\"[role=dialog]\").length","args":[]}' -TimeoutSec 10).value
  if ($dialogCount -ne 0) { throw 'Escape did not close the command palette.' }

  $libraryButton = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element" `
    -ContentType 'application/json' -Body '{"using":"xpath","value":"//button[normalize-space(.)=\"Library\"]"}' -TimeoutSec 10
  $libraryButtonId = $libraryButton.value.'element-6066-11e4-a52e-4f735466cecf'
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element/$libraryButtonId/click" `
    -ContentType 'application/json' -Body '{}' -TimeoutSec 10 | Out-Null
  Start-Sleep -Milliseconds 250
  $savedExportState = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
    -ContentType 'application/json' -Body '{"script":"const button = Array.from(document.querySelectorAll(\"button\")).find((candidate) => candidate.textContent?.trim() === \"Export saved items\"); return { heading: document.querySelector(\"h1\")?.textContent, disabled: button?.disabled ?? null };","args":[]}' -TimeoutSec 10).value
  if ($savedExportState.heading -ne 'Library.' -or $savedExportState.disabled -ne $true) {
    throw 'An empty native Library did not keep portable saved-item export unavailable.'
  }

  $sourcesButton = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element" `
    -ContentType 'application/json' -Body '{"using":"xpath","value":"//button[normalize-space(.)=\"Sources\"]"}' -TimeoutSec 10
  $sourcesButtonId = $sourcesButton.value.'element-6066-11e4-a52e-4f735466cecf'
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element/$sourcesButtonId/click" `
    -ContentType 'application/json' -Body '{}' -TimeoutSec 10 | Out-Null
  Start-Sleep -Milliseconds 250
  $mastodonPreflight = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
    -ContentType 'application/json' -Body '{"script":"const input = document.querySelector(\"#mastodon-instance-url\"); const button = Array.from(document.querySelectorAll(\"button\")).find((candidate) => candidate.textContent?.trim() === \"Check compatibility\"); const connect = Array.from(document.querySelectorAll(\"button\")).some((candidate) => /connect mastodon/i.test(candidate.textContent ?? \"\")); return { heading: document.querySelector(\"h1\")?.textContent, hasInput: Boolean(input), disabled: button?.disabled ?? null, hasConnect: connect };","args":[]}' -TimeoutSec 10).value
  if ($mastodonPreflight.heading -ne 'Your sources.' -or -not $mastodonPreflight.hasInput -or $mastodonPreflight.disabled -or $mastodonPreflight.hasConnect) {
    throw 'Mastodon preflight was not present as a non-connecting native-only control.'
  }

  $settingsButton = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element" `
    -ContentType 'application/json' -Body '{"using":"xpath","value":"//button[normalize-space(.)=\"Privacy \u0026 settings\"]"}' -TimeoutSec 10
  $settingsButtonId = $settingsButton.value.'element-6066-11e4-a52e-4f735466cecf'
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element/$settingsButtonId/click" `
    -ContentType 'application/json' -Body '{}' -TimeoutSec 10 | Out-Null
  Start-Sleep -Milliseconds 250

  $restoreButton = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element" `
    -ContentType 'application/json' -Body '{"using":"xpath","value":"//button[normalize-space(.)=\"Restore backup\"]"}' -TimeoutSec 10
  $restoreButtonId = $restoreButton.value.'element-6066-11e4-a52e-4f735466cecf'
  $scrollTarget = @{ script = 'arguments[0].scrollIntoView({block: "center", inline: "nearest"});'; args = @($restoreButton.value) } |
    ConvertTo-Json -Depth 4 -Compress
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
    -ContentType 'application/json' -Body $scrollTarget -TimeoutSec 10 | Out-Null
  Start-Sleep -Milliseconds 100
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element/$restoreButtonId/click" `
    -ContentType 'application/json' -Body '{}' -TimeoutSec 10 | Out-Null
  Start-Sleep -Milliseconds 250
  $restoreDialog = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element" `
    -ContentType 'application/json' -Body '{"using":"css selector","value":"[role=dialog]"}' -TimeoutSec 10
  $restoreDialogId = $restoreDialog.value.'element-6066-11e4-a52e-4f735466cecf'
  $restoreDialogText = (Invoke-RestMethod -Method Get -Uri "http://127.0.0.1:$Port/session/$sessionId/element/$restoreDialogId/text" -TimeoutSec 10).value
  if ($restoreDialogText -notmatch 'Restore a local backup') {
    throw 'Restore backup did not open the in-app confirmation dialog.'
  }
  $restoreFocus = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
    -ContentType 'application/json' -Body '{"script":"return document.activeElement?.innerText","args":[]}' -TimeoutSec 10).value
  if ($restoreFocus -notmatch 'Choose backup and restore') {
    throw 'Restore confirmation did not move focus to its explicit destructive action.'
  }
  Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/actions" `
    -ContentType 'application/json' -Body $escape -TimeoutSec 10 | Out-Null
  Start-Sleep -Milliseconds 250
  $restoreDialogCount = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
    -ContentType 'application/json' -Body '{"script":"return document.querySelectorAll(\"[role=dialog]\").length","args":[]}' -TimeoutSec 10).value
  if ($restoreDialogCount -ne 0) { throw 'Escape did not close the restore confirmation.' }

  if ($ReadingScale) {
    $readingScaleSelector = "input[name=reading-scale][value='$ReadingScale']"
    $readingScaleLookup = @{ using = 'css selector'; value = $readingScaleSelector } |
      ConvertTo-Json -Compress
    $readingScaleInput = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/element" `
      -ContentType 'application/json' -Body $readingScaleLookup -TimeoutSec 10
    $scrollScale = @{ script = 'arguments[0].scrollIntoView({block: "center", inline: "nearest"});'; args = @($readingScaleInput.value) } |
      ConvertTo-Json -Depth 4 -Compress
    Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
      -ContentType 'application/json' -Body $scrollScale -TimeoutSec 10 | Out-Null
    $activateReadingScale = @{ script = 'const input = document.querySelector(arguments[0]); input?.closest("label")?.click(); return Boolean(input);'; args = @($readingScaleSelector) } |
      ConvertTo-Json -Depth 4 -Compress
    $readingScaleActivated = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
      -ContentType 'application/json' -Body $activateReadingScale -TimeoutSec 10).value
    if (-not $readingScaleActivated) { throw "Reading scale $ReadingScale control was not reachable." }
    Start-Sleep -Milliseconds 500
    $readingScaleState = (Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/session/$sessionId/execute/sync" `
      -ContentType 'application/json' -Body '{"script":"const selected = document.querySelector(\"input[name=reading-scale]:checked\"); return { value: selected?.value ?? null, saved: localStorage.getItem(\"web.presentation.zoom\") };","args":[]}' -TimeoutSec 10).value
    if ($readingScaleState.value -ne "$ReadingScale" -or $readingScaleState.saved -ne "$ReadingScale") {
      throw "Reading scale $ReadingScale was not selected and saved in the native WebView."
    }
    if ($readingScaleScreenshotOutputPath) {
      New-Item -ItemType Directory -Force -Path (Split-Path $readingScaleScreenshotOutputPath -Parent) | Out-Null
      $readingScaleScreenshot = (Invoke-RestMethod -Method Get -Uri "http://127.0.0.1:$Port/session/$sessionId/screenshot" -TimeoutSec 10).value
      [IO.File]::WriteAllBytes($readingScaleScreenshotOutputPath, [Convert]::FromBase64String($readingScaleScreenshot))
    }
  }

  [pscustomobject]@{
    Heading = $rendered.heading
    Shortcut = $rendered.shortcut
    CommandPalette = 'opened and closed through native WebDriver'
    SavedExport = 'empty-library safeguard verified through native WebDriver'
    MastodonPreflight = 'metadata-only control rendered without a Connect action through native WebDriver'
    RestoreConfirmation = 'opened, focused, and dismissed through native WebDriver'
    Screenshot = $screenshotOutputPath
    Window = if ($WindowWidth -gt 0) { "$WindowWidth x $WindowHeight" } else { 'driver default' }
    Theme = $rendered.theme
    ReadingScale = $readingScaleState
    ReadingScaleScreenshot = $readingScaleScreenshotOutputPath
    ZoomInSteps = $ZoomInSteps
    CssViewportWidth = $rendered.viewportWidth
    ZoomMetrics = $rendered.zoomMetrics
  }
} finally {
  if ($sessionId) {
    try { Invoke-WebRequest -Method Delete -Uri "http://127.0.0.1:$Port/session/$sessionId" -TimeoutSec 10 | Out-Null } catch {}
  }
  if ($driver -and -not $driver.HasExited) { Stop-Process -Id $driver.Id -ErrorAction SilentlyContinue }
  foreach ($processId in (Get-ProcessIdsForPath $applicationPath 'web-social-digest.exe' | Where-Object { $_ -notin $priorAppPids })) {
    Stop-Process -Id $processId -ErrorAction SilentlyContinue
  }
  foreach ($processId in (Get-ProcessIdsForPath $nativeDriverPath 'msedgedriver.exe' | Where-Object { $_ -notin $priorNativeDriverPids })) {
    Stop-Process -Id $processId -ErrorAction SilentlyContinue
  }
}
