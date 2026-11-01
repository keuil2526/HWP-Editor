# hwpedit <-> 한글 2022 검증 작업기 (WinBoat 안의 Windows에서 실행)
#
# inbox\<id>.job.json 과 inbox\<id>.<hwp|hwpx> 를 받아 한글 자동화(HwpObject)로 열고
# outbox\ 에 결과를 씁니다:
#   <id>.result.json  열기 성공 여부, 쪽 수, 걸린 시간, 오류
#   <id>.txt          한글이 추출한 본문 텍스트 (GetTextFile "TEXT")
#   <id>.pdf          한글이 출력한 PDF
#   <id>.resaved.<ext> 한글이 같은 형식으로 다시 저장한 파일
#
#
# UI 탐색 작업 ({"op":"ui","steps":[...]}): 한글 창을 앞으로 가져와 단계를 차례로 실행하고
# 캡처를 outbox\<id>.<이름>.png 로 씁니다. 단계:
#   {"new":true}            빈 새 문서
#   {"text":"..."}          글자 입력 (InsertText)
#   {"action":"TableCreate"} 한글 명령(HAction) 실행 (대화상자가 없는 명령)
#   {"dialog":"CharShape","shot":"이름"} 대화상자를 여는 명령: 열린 뒤 캡처하고 ESC로 닫음
#   {"keys":"%"}            키 입력 (SendKeys 문법: % = Alt, ^ = Ctrl, {ESC} 등)
#   {"sleep":500}           기다리기 (ms)
#   {"shot":"이름"}          화면 캡처
#   {"maximize":true}       한글 창 최대화
#   {"click":[x,y]}         화면 좌표 (x,y) 왼쪽 클릭 (캡처와 같은 좌표계)
#   {"hover":[x,y]}         마우스만 옮기기 (도움말 풍선 확인용)
#   {"dialog":..., "close":"{ENTER}"}  대화상자를 ESC 대신 이 키로 닫기
#
# 감시: 별도 작업이 5초마다 outbox\worker.status 에 지금 단계를 씁니다. 한 단계가 45초를
# 넘기면 화면을 outbox\stuck-<id>-<단계>.png 로 캡처하고, 한글 명령에서 150초를 넘기면
# 작업기가 띄운 한글만 끝내서 다음 작업으로 넘어갑니다 (outbox\worker-watchdog.txt 에 기록).
#
# 실행: powershell -ExecutionPolicy Bypass -File \\host.lan\Data\hwpedit-bridge\hancom-worker.ps1
# 중지: 창에서 Ctrl+C

$ErrorActionPreference = 'Stop'
$root   = $PSScriptRoot
$inbox  = Join-Path $root 'inbox'
$outbox = Join-Path $root 'outbox'
$work   = Join-Path $env:TEMP 'hwpedit-bridge'
New-Item -ItemType Directory -Force -Path $inbox, $outbox, $work | Out-Null

Add-Type -AssemblyName System.Windows.Forms, System.Drawing, Microsoft.VisualBasic
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class HwpeditInput {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
    [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll")] static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll")] static extern bool SetConsoleMode(IntPtr h, uint mode);
    // A click in the console starts a QuickEdit selection, which pauses the script
    // until Esc/Enter is pressed there. Turn it off so a stray click cannot stall jobs.
    public static void NoQuickEdit() {
        IntPtr h = GetStdHandle(-10);
        uint m;
        if (GetConsoleMode(h, out m)) SetConsoleMode(h, (m & ~0x40u) | 0x80u);
    }
    public static void Click(int x, int y) {
        SetCursorPos(x, y);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
        mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    }
}
'@
try { [HwpeditInput]::NoQuickEdit() } catch {}

function Log($msg) { Write-Host ("[{0:HH:mm:ss}] {1}" -f (Get-Date), $msg) }

$stateFile = Join-Path $work 'worker.state'
$script:hwpPid = 0
function Set-Stage($stage, $id) {
    try { [IO.File]::WriteAllText($stateFile, ("{0}|{1}|{2}|{3}" -f $stage, $id, (Get-Date).Ticks, $script:hwpPid)) } catch {}
}

# Runs in its own process, so it keeps working while a Hwp call blocks the main loop.
$Watchdog = {
    param($stateFile, $outbox, $parent)
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    $log = Join-Path $outbox 'worker-watchdog.txt'
    $shot = ''; $killed = ''
    while (Get-Process -Id $parent -ErrorAction SilentlyContinue) {
        Start-Sleep -Seconds 5
        try { $stage, $id, $ticks, $hwpPid = [IO.File]::ReadAllText($stateFile).Split('|') } catch { continue }
        $age = [int](((Get-Date).Ticks - [long]$ticks) / 1e7)
        $key = "$stage|$id|$ticks"
        try { Set-Content -Encoding UTF8 (Join-Path $outbox 'worker.status') ("{0:HH:mm:ss} stage={1} id={2} for={3}s hwp={4}" -f (Get-Date), $stage, $id, $age, $hwpPid) } catch {}
        if ($age -ge 45 -and $shot -ne $key) {
            $shot = $key
            try {
                $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
                $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
                $g = [System.Drawing.Graphics]::FromImage($bmp)
                $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
                $bmp.Save((Join-Path $outbox ("stuck-{0}-{1}.png" -f $id, $stage)), [System.Drawing.Imaging.ImageFormat]::Png)
                $g.Dispose(); $bmp.Dispose()
            } catch {}
            Add-Content -Encoding UTF8 $log ("{0:HH:mm:ss} {1} {2}: no progress for {3}s, screen saved" -f (Get-Date), $id, $stage, $age)
        }
        # Only Hwp calls are cut short, and only the Hwp instance this worker started.
        if ($age -ge 150 -and $stage -ne 'idle' -and $killed -ne $key -and [int]$hwpPid -gt 0) {
            $killed = $key
            Stop-Process -Id ([int]$hwpPid) -Force -ErrorAction SilentlyContinue
            Add-Content -Encoding UTF8 $log ("{0:HH:mm:ss} {1} {2}: stopped Hwp {3}" -f (Get-Date), $id, $stage, $hwpPid)
        }
    }
}

function New-Hwp {
    $h = New-Object -ComObject 'HWPFrame.HwpObject'
    # 보안 모듈이 등록돼 있으면 파일 접근 확인 창 없이 엽니다. 없으면 처음 한 번 '모두 허용'을 눌러 주세요.
    try { [void]$h.RegisterModule('FilePathCheckDLL', 'FilePathCheckerModule') } catch {}
    # 메시지 상자에 기본 단추를 자동으로 누릅니다 (한컴 자동화 문서의 SetMessageBoxMode).
    try { [void]$h.SetMessageBoxMode(0x2FFF1) } catch {}
    try { $h.XHwpWindows.Item(0).Visible = $true } catch {}
    $p = Get-Process -Name Hwp -ErrorAction SilentlyContinue | Sort-Object StartTime -Descending | Select-Object -First 1
    $script:hwpPid = if ($p) { $p.Id } else { 0 }
    return $h
}

function Show-Hwp {
    try { $hwp.XHwpWindows.Item(0).Visible = $true } catch {}
    $p = Get-Process -Name Hwp -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } |
        Sort-Object StartTime -Descending | Select-Object -First 1
    if ($p) { try { [Microsoft.VisualBasic.Interaction]::AppActivate($p.Id) } catch {} }
    Start-Sleep -Milliseconds 400
}

function Save-Shot($path) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

# A dialog command blocks inside HAction.Run until the dialog closes, so a background
# job captures the screen and presses ESC while we wait.
$DialogCloser = {
    param($png, $ms, $keys)
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    Start-Sleep -Milliseconds $ms
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
    $bmp.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    if ($keys) { [System.Windows.Forms.SendKeys]::SendWait($keys); Start-Sleep -Milliseconds 400 }
    else { for ($i = 0; $i -lt 3; $i++) { [System.Windows.Forms.SendKeys]::SendWait('{ESC}'); Start-Sleep -Milliseconds 400 } }
}

function Invoke-UiJob($job, $id, $result) {
    $log = Join-Path $outbox "$id.progress.txt"
    Set-Content -Encoding UTF8 $log "start"
    Show-Hwp
    $shots = @()
    $n = 0
    foreach ($s in $job.steps) {
        $n++
        Set-Stage "ui$n" $id
        Add-Content -Encoding UTF8 $log ("{0}: {1}" -f $n, ($s | ConvertTo-Json -Compress))
        if ($s.dialog) {
            $png = Join-Path $outbox ("{0}.{1}.png" -f $id, $s.shot)
            $j = Start-Job -ScriptBlock $DialogCloser -ArgumentList $png, 1800, ([string]$s.close)
            Show-Hwp
            try { [void]$hwp.HAction.Run([string]$s.dialog) } catch { Add-Content $log $_.Exception.Message }
            [void](Wait-Job $j -Timeout 20); Remove-Job -Force $j
            if (Test-Path $png) { $shots += [string]$s.shot }
            continue
        }
        if ($s.new) { [void]$hwp.Clear(1) }
        elseif ($s.text) {
            $act = $hwp.CreateAction('InsertText'); $set = $act.CreateSet(); [void]$act.GetDefault($set)
            $set.SetItem('Text', [string]$s.text); [void]$act.Execute($set)
        }
        elseif ($s.action) { $ok = $hwp.HAction.Run([string]$s.action); Log ("  action {0}: {1}" -f $s.action, $ok) }
        elseif ($s.keys) { Show-Hwp; [System.Windows.Forms.SendKeys]::SendWait([string]$s.keys) }
        elseif ($s.maximize) {
            $p = Get-Process -Name Hwp -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
            if ($p) { [void][HwpeditInput]::ShowWindow($p.MainWindowHandle, 3) }
        }
        elseif ($s.click) { [HwpeditInput]::Click([int]$s.click[0], [int]$s.click[1]) }
        elseif ($s.hover) { [void][HwpeditInput]::SetCursorPos([int]$s.hover[0], [int]$s.hover[1]) }
        elseif ($s.sleep) { Start-Sleep -Milliseconds ([int]$s.sleep) }
        elseif ($s.shot) {
            $png = Join-Path $outbox ("{0}.{1}.png" -f $id, $s.shot)
            Save-Shot $png
            $shots += [string]$s.shot
        }
    }
    Add-Content -Encoding UTF8 $log "done"
    $result.shots = $shots
    $result.ok = $true
}

Log "한글 자동화를 시작합니다: $root"
$hwp = New-Hwp
Set-Stage 'idle' ''
$watch = Start-Job -ScriptBlock $Watchdog -ArgumentList $stateFile, $outbox, $PID
Log "준비됐습니다. inbox 를 기다립니다."

while ($true) {
    Set-Stage 'idle' ''
    $jobs = Get-ChildItem -Path $inbox -Filter '*.job.json' -ErrorAction SilentlyContinue | Sort-Object Name
    foreach ($jobFile in $jobs) {
        $id = $jobFile.Name -replace '\.job\.json$', ''
        $result = [ordered]@{ id = $id; ok = $false; hancom = 'Hwp 2022 HwpObject' }
        $job = $null
        try {
            $job = Get-Content -Raw -Encoding UTF8 $jobFile.FullName | ConvertFrom-Json
            if ($job.op -eq 'ui') { Invoke-UiJob $job $id $result; continue }
            $src = Join-Path $inbox $job.file
            $ext = [IO.Path]::GetExtension($job.file).TrimStart('.').ToLower()
            # 네트워크 경로가 아니라 로컬 사본으로 작업합니다.
            $local = Join-Path $work ("$id.$ext")
            Set-Stage 'copy' $id
            Copy-Item -Force $src $local
            Set-Stage 'open' $id
            $sw = [Diagnostics.Stopwatch]::StartNew()
            $opened = $hwp.Open($local, '', 'forceopen:true;suspendpassword:true;versionwarning:false')
            $result.open_ms = $sw.ElapsedMilliseconds
            $result.opened = [bool]$opened
            if ($opened) {
                Set-Stage 'pages' $id
                $result.pages = $hwp.PageCount
                if ($job.ops -contains 'text') {
                    Set-Stage 'text' $id
                    $text = $hwp.GetTextFile('TEXT', '')
                    [IO.File]::WriteAllText((Join-Path $outbox "$id.txt"), $text, [Text.UTF8Encoding]::new($false))
                }
                if ($job.ops -contains 'pdf') {
                    Set-Stage 'pdf' $id
                    $pdf = Join-Path $work "$id.pdf"
                    $result.pdf = [bool]$hwp.SaveAs($pdf, 'PDF', '')
                    if ($result.pdf) { Copy-Item -Force $pdf (Join-Path $outbox "$id.pdf") }
                }
                if ($job.ops -contains 'resave') {
                    Set-Stage 'resave' $id
                    $fmt = if ($ext -eq 'hwpx') { 'HWPX' } else { 'HWP' }
                    $re = Join-Path $work "$id.resaved.$ext"
                    $result.resave = [bool]$hwp.SaveAs($re, $fmt, '')
                    if ($result.resave) { Copy-Item -Force $re (Join-Path $outbox "$id.resaved.$ext") }
                }
                $result.ok = $true
            }
        } catch {
            $result.error = $_.Exception.Message
        } finally {
            Set-Stage 'clear' $id
            try { [void]$hwp.Clear(1) } catch {
                # 한글이 응답하지 않으면 새로 띄웁니다.
                try { $hwp.Quit() } catch {}
                $hwp = New-Hwp
            }
            Set-Stage 'write' $id
            $result | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $outbox "$id.result.json")
            Remove-Item -Force $jobFile.FullName
            if ($job -and $job.file) { Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $inbox $job.file) }
            Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $work "$id.*")
            Log ("{0}: opened={1} pages={2} {3}" -f $id, $result.opened, $result.pages, $result.error)
        }
    }
    Start-Sleep -Milliseconds 700
}
