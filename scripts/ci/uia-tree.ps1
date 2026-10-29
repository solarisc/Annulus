# Reads a running Annulus window through UI Automation, the API Windows screen readers use, and
# checks that the accessibility tree (annulus_ui::access, served by AccessKit) is what they get:
# the named controls are there with the right control types, and no password field exposes a
# value. Exits non-zero on a problem. Usage:
#   scripts/ci/uia-tree.ps1 -Bin target/debug/annulus-app.exe [-Out tree.txt]
param(
    [Parameter(Mandatory = $true)][string]$Bin,
    [string]$Out = ''
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$app = Start-Process -FilePath $Bin -ArgumentList '--preview' -PassThru
try {
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $byPid = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $app.Id)
    $window = $null
    for ($i = 0; $i -lt 60 -and -not $window; $i++) {
        Start-Sleep -Milliseconds 250
        $window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $byPid)
    }
    if (-not $window) { throw 'the Annulus window never appeared to UI Automation' }
    # The tree is built when UI Automation first asks; give it a frame.
    Start-Sleep -Milliseconds 500
    $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition)
    $lines = foreach ($element in $all) {
        $info = $element.Current
        $value = ''
        $pattern = $null
        if ($element.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
            $value = $pattern.Current.Value
        }
        '{0} | {1} | {2} | {3}' -f $info.ControlType.ProgrammaticName, $info.Name, $info.IsPassword, $value
    }
    if ($Out) { $lines | Set-Content -Path $Out -Encoding utf8 }
    $lines | Select-Object -First 40
    "elements: $($all.Count)"
    $problems = @()
    foreach ($want in @('ControlType.Button | New host', 'ControlType.Edit | Search hosts', 'ControlType.TabItem | Vault', 'ControlType.ListItem | prod-db')) {
        if (-not ($lines | Where-Object { $_.StartsWith($want) })) { $problems += "missing: $want" }
    }
    foreach ($line in $lines) {
        $parts = $line -split ' \| '
        if ($parts[2] -eq 'True' -and $parts[3]) { $problems += "a password field exposes a value: $($parts[1])" }
    }
    if ($problems) { $problems; exit 1 }
    'ok'
}
finally {
    Stop-Process -Id $app.Id -ErrorAction SilentlyContinue
}
