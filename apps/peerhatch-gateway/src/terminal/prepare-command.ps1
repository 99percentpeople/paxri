# Prepare the original Windows PowerShell 5.1 command. Wrapping it in a
# nested script block would change $input and the command's implicit exit code.
$ErrorActionPreference = 'Stop'
$source = $env:PEERHATCH_PS_SOURCE
$setup = $env:PEERHATCH_PS_SETUP
$tokens = $errorsFound = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errorsFound)
$pos = 0
if ($ast.UsingStatements.Count) { $pos = $ast.UsingStatements[-1].Extent.EndOffset }
if ($ast.ParamBlock) { $pos = $ast.ParamBlock.Extent.EndOffset }
if ($ast.DynamicParamBlock -or $ast.BeginBlock) {
    $block = if ($ast.DynamicParamBlock) { $ast.DynamicParamBlock } else { $ast.BeginBlock }
    $brace = $tokens | Where-Object { $_.Extent.StartOffset -ge $block.Extent.StartOffset -and $_.Kind -eq 'LCurly' } | Select-Object -First 1
    if ($brace) { $pos = $brace.Extent.EndOffset }
} elseif ($ast.ProcessBlock -or $ast.CleanBlock -or ($ast.EndBlock -and !$ast.EndBlock.Unnamed)) {
    $setup = "begin { $setup }"
}
if (!$errorsFound) { $source = $source.Insert($pos, [Environment]::NewLine + $setup + [Environment]::NewLine) }
$bytes = [Text.Encoding]::UTF8.GetBytes($source)
[Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
