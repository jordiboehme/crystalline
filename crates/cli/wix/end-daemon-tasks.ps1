$schtasks = Join-Path $env:SystemRoot 'System32\schtasks.exe'
Get-ScheduledTask -ErrorAction SilentlyContinue |
    Where-Object {
        ($_.TaskPath -eq '\Crystalline\' -and $_.TaskName -eq 'Daemon') -or
        ($_.TaskPath -eq '\' -and $_.TaskName -like 'Crystalline Daemon for *')
    } |
    ForEach-Object { & $schtasks /End /TN ($_.TaskPath + $_.TaskName) }
exit 0
