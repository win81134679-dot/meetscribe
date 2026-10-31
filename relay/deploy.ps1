# 部署「傳到其他裝置」的中轉站到你的 Cloudflare 帳號（Workers＋R2＋D1）。重複執行沒關係：已經建好的會略過。
# 用法：powershell -ExecutionPolicy Bypass -File relay\deploy.ps1
# 第一次會開瀏覽器請你登入 Cloudflare 並按「Allow」。R2 第一次要在 Cloudflare 網頁上綁信用卡或 PayPal（驗證用，免費額度內不收費）。

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
Push-Location $here
try {
    $name = 'meetscribe-relay'
    if (-not (Test-Path (Join-Path $here 'node_modules\wrangler'))) {
        Write-Host '安裝 wrangler…'
        npm install --no-fund --no-audit | Out-Host
    }

    # 1. 登入
    $who = (npx wrangler whoami 2>&1) -join "`n"
    if ($env:CLOUDFLARE_API_TOKEN -and ($LASTEXITCODE -or $who -match 'not authenticated|You are not logged in|Invalid|error')) {
        Write-Host $who
        throw 'Cloudflare 不接受這個 API 權杖（貼錯了，或權限不夠）'
    }
    if ($who -match 'not authenticated|You are not logged in') {
        Write-Host '請在開啟的瀏覽器登入 Cloudflare 並按「Allow」…'
        npx wrangler login | Out-Host
        $who = (npx wrangler whoami 2>&1) -join "`n"
        if ($who -match 'not authenticated|You are not logged in') { throw 'Cloudflare 登入沒有完成' }
    }
    Write-Host '已登入 Cloudflare'

    # 2. R2 儲存桶
    $buckets = (npx wrangler r2 bucket list 2>&1) -join "`n"
    if ($buckets -match 'enable R2|10042|Please enable') {
        throw "R2 還沒啟用：到 https://dash.cloudflare.com/ 左側「R2 物件儲存」按「開始使用」，綁信用卡或 PayPal 後再執行一次"
    }
    if ($buckets -notmatch "name:\s+$name\b") {
        Write-Host "建立 R2 儲存桶 $name…"
        $out = (npx wrangler r2 bucket create $name 2>&1) -join "`n"
        Write-Host $out
        if ($LASTEXITCODE -and $out -notmatch 'already exists') {
            if ($out -match 'enable R2|10042') {
                throw "R2 還沒啟用：到 https://dash.cloudflare.com/ 左側「R2 物件儲存」按「開始使用」，綁信用卡或 PayPal 後再執行一次"
            }
            throw '建立 R2 儲存桶失敗'
        }
    }

    # 保險：物件 1 天後自動刪除（正常 3 小時內就刪了；這條只接漏刪的，刪除不收費）
    $rules = (npx wrangler r2 bucket lifecycle list $name 2>&1) -join "`n"
    if ($rules -notmatch 'auto-delete') {
        $out = (npx wrangler r2 bucket lifecycle add $name auto-delete --expire-days 1 --force 2>&1) -join "`n"
        if ($LASTEXITCODE -and $out -notmatch 'already exists') { Write-Host $out; throw '設定 R2 自動刪除規則失敗' }
    }
    Write-Host 'R2 自動刪除規則：1 天'

    # 3. D1 資料庫（把 ID 寫進 wrangler.toml）
    $list = npx wrangler d1 list --json 2>$null | Out-String
    if ($LASTEXITCODE) {
        $err = (npx wrangler d1 list 2>&1) -join "`n"
        if ($err -match '10000|Authentication error') {
            throw "權杖沒有 D1 權限：到 https://dash.cloudflare.com/profile/api-tokens 在這個權杖按「編輯」，加一列「帳戶 / D1 / 編輯」後再執行一次（權杖不用重貼新的，內容不變）"
        }
        Write-Host $err
        throw '讀取 D1 資料庫清單失敗'
    }
    $db = $null
    try { $db = ($list | ConvertFrom-Json) | Where-Object { $_.name -eq $name } | Select-Object -First 1 } catch {}
    if (-not $db) {
        Write-Host "建立 D1 資料庫 $name…"
        npx wrangler d1 create $name | Out-Host
        $list = npx wrangler d1 list --json 2>$null | Out-String
        $db = ($list | ConvertFrom-Json) | Where-Object { $_.name -eq $name } | Select-Object -First 1
        if (-not $db) { throw '建立 D1 資料庫失敗' }
    }
    $toml = Join-Path $here 'wrangler.toml'
    $text = [IO.File]::ReadAllText($toml)
    $text = [regex]::Replace($text, 'database_id = "[^"]*"', "database_id = `"$($db.uuid)`"")
    [IO.File]::WriteAllText($toml, $text, (New-Object Text.UTF8Encoding $false))
    Write-Host "D1 資料庫：$($db.uuid)"

    # 4. 資料表
    npx wrangler d1 execute $name --remote --file schema.sql --yes | Out-Null
    if ($LASTEXITCODE) { throw '建立資料表失敗' }
    Write-Host '資料表已就緒'

    # 5. 部署
    $deploy = (npx wrangler deploy 2>&1) -join "`n"
    Write-Host $deploy
    if ($LASTEXITCODE) {
        if ($deploy -match '10000|Authentication error') {
            throw '權杖沒有 Workers 權限：在這個權杖按「編輯」，確認有「帳戶 / Workers 指令碼 / 編輯」後再執行一次'
        }
        throw '部署失敗（訊息在上面）'
    }
    $url = [regex]::Match($deploy, 'https://[a-z0-9.-]+\.workers\.dev').Value
    if (-not $url) { throw '部署完成但找不到網址' }

    # 6. 健康檢查
    $health = Invoke-RestMethod "$url/v1/health" -TimeoutSec 30
    if (-not $health.ok) { throw "中轉站沒有回應：$url" }
    [IO.File]::WriteAllText((Join-Path $here 'deployed-url.txt'), $url, (New-Object Text.UTF8Encoding $false))
    Write-Host ''
    Write-Host "完成：$url" -ForegroundColor Green
    Write-Host '把這個網址告訴 Claude，它會寫進 App 的預設值並發佈新版。'
}
finally {
    Pop-Location
}
