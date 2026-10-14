# 把 Tauri 壳数据根的设置行（含登录账号）同步到 GPUI 壳数据根。
#
# 背景：两壳数据根必须分离（库锁互斥，见 hongguo-store/paths.rs 头注释），
# 但登录账号存在 settings 表 id=1 那一行的 JSON 里——GPUI 壳启动时已会
# 装载本根 settings（main.rs build_app_state），把 Tauri 登录后的那一行
# 拷过来，GPUI 壳重启即持有同一登录态（发评论/回复/点赞/删除自己的评论）。
#
# 用法：先在 Tauri 版登录 → 关闭两壳 → 运行本脚本 → 启动 GPUI 壳。
# 幂等：重复执行以 Tauri 侧为准覆盖。依赖 sqlite3 命令行
#（winget install SQLite.SQLite）。

$ErrorActionPreference = 'Stop'

$tauriDb = Join-Path $env:APPDATA 'hongguo-downloader\hongguo.db'
$gpuiDb  = Join-Path $env:APPDATA 'hongguo-gpui\hongguo.db'

foreach ($f in @($tauriDb, $gpuiDb)) {
    if (-not (Test-Path $f)) {
        Write-Error "找不到数据库：$f（两壳都先启动过一次才会生成）"
        exit 1
    }
}

$sqlite = Get-Command sqlite3 -ErrorAction SilentlyContinue
if (-not $sqlite) {
    Write-Error '需要 sqlite3 命令行工具（winget install SQLite.SQLite）'
    exit 1
}

$json = (& $sqlite $tauriDb 'SELECT json FROM settings WHERE id = 1;') -join "`n"
if (-not $json) {
    Write-Error 'Tauri 侧没有设置行（未登录过？先在 Tauri 版登录）'
    exit 1
}

# JSON 里的单引号按 SQL 字面量规则翻倍
$escaped = $json -replace "'", "''"
& $sqlite $gpuiDb "INSERT INTO settings (id, json) VALUES (1, '$escaped') ON CONFLICT(id) DO UPDATE SET json = excluded.json;"

$acct = ($json | ConvertFrom-Json).account
if ($acct -and $acct.user_id) {
    Write-Output "已同步登录态到 GPUI 数据根（user_id=$($acct.user_id)）。启动 GPUI 壳即生效。"
} else {
    Write-Warning 'Tauri 侧 settings 里没有账号（未登录？）——已同步其余设置。'
}
