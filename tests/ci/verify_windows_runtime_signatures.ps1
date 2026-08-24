[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string]$Target,

  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string]$TemporaryDirectory,

  [ValidateNotNullOrEmpty()]
  [string]$DistDirectory = "dist",

  [Parameter(Mandatory = $true)]
  [ValidateNotNullOrEmpty()]
  [string]$RuntimePrefix,

  [ValidateNotNullOrEmpty()]
  [string]$CsExecutable = ".\target\release\cs.exe"
)

$ErrorActionPreference = "Stop"

function Get-ByteRange {
  param(
    [byte[]]$Bytes,
    [long]$Offset,
    [long]$Length
  )
  if (
    $Offset -lt 0 -or
    $Length -lt 0 -or
    $Offset + $Length -gt $Bytes.LongLength -or
    $Offset -gt [int]::MaxValue -or
    $Length -gt [int]::MaxValue
  ) {
    throw "Byte range is outside the PE file"
  }
  $result = [byte[]]::new([int]$Length)
  [Buffer]::BlockCopy($Bytes, [int]$Offset, $result, 0, [int]$Length)
  return ,$result
}

function Get-Sha256 {
  param([byte[]]$Bytes)
  $sha256 = [Security.Cryptography.SHA256]::Create()
  try {
    return ,$sha256.ComputeHash($Bytes)
  }
  finally {
    $sha256.Dispose()
  }
}

function Flip-FileByte {
  param(
    [string]$Path,
    [long]$Offset
  )
  $stream = [System.IO.File]::Open(
    $Path,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::ReadWrite
  )
  try {
    $stream.Position = $Offset
    $value = $stream.ReadByte()
    if ($value -lt 0) {
      throw "Mutation offset is outside the PE file"
    }
    $stream.Position = $Offset
    $stream.WriteByte($value -bxor 1)
  }
  finally {
    $stream.Dispose()
  }
}

function Assert-RuntimeReadFailure {
  param(
    [string]$Binary,
    [string]$Info,
    [string]$Expected,
    [string]$Label
  )
  $packageDir = Join-Path $TemporaryDirectory (
    "runtime-read-$([guid]::NewGuid())"
  )
  New-Item -ItemType Directory -Path $packageDir | Out-Null
  Write-Host "Checking runtime-data rejection: $Label"
  $previousErrorActionPreference = $ErrorActionPreference
  try {
    $ErrorActionPreference = "Continue"
    $output = & $CsExecutable package-update `
      --info $Info `
      --binary $Binary `
      --out-dir $packageDir 2>&1 | Out-String
    $status = $LASTEXITCODE
  }
  finally {
    $ErrorActionPreference = $previousErrorActionPreference
  }
  if ($status -eq 0 -or $output -notmatch [regex]::Escape($Expected)) {
    throw "$Label was not rejected as expected: $output"
  }
  # GitHub's PowerShell wrapper exits with the final native-command status.
  $global:LASTEXITCODE = 0
}

function Assert-SignedRuntimeExecutes {
  param(
    [string]$Binary,
    [string]$Prefix
  )
  $hadConfiguredPrefix = Test-Path Env:CONDA_SHIP_PREFIX
  $configuredPrefix = $env:CONDA_SHIP_PREFIX
  try {
    $env:CONDA_SHIP_PREFIX = $Prefix
    $null = & $Binary --help
    if ($LASTEXITCODE -ne 0) {
      throw "The signed runtime did not execute"
    }
  }
  finally {
    if ($hadConfiguredPrefix) {
      $env:CONDA_SHIP_PREFIX = $configuredPrefix
    }
    else {
      Remove-Item Env:CONDA_SHIP_PREFIX -ErrorAction SilentlyContinue
    }
  }
}

function Assert-AuthenticodeIntegrity {
  param(
    [string]$Path,
    [string]$Thumbprint,
    [string]$Label
  )
  $signature = Get-AuthenticodeSignature -LiteralPath $Path
  Write-Host "$Label Authenticode status: $($signature.Status)"
  if (
    -not $signature.SignerCertificate -or
    $signature.SignerCertificate.Thumbprint -ne $Thumbprint
  ) {
    throw "$Label does not have the expected Authenticode signer"
  }
  $untrustedRoot = (
    $signature.Status -eq "UnknownError" -and
    $signature.StatusMessage -match "root certificate.+not trusted"
  )
  if (
    $signature.Status -notin @("Valid", "NotTrusted") -and
    -not $untrustedRoot
  ) {
    throw "$Label does not have an intact Authenticode signature: $($signature.StatusMessage)"
  }
}

function Assert-AuthenticodeHashMismatch {
  param(
    [string]$Path,
    [string]$Label
  )
  $signature = Get-AuthenticodeSignature -LiteralPath $Path
  Write-Host "$Label Authenticode status: $($signature.Status)"
  if ($signature.Status -ne "HashMismatch") {
    throw "$Label was not reported as an Authenticode hash mismatch"
  }
}

if (-not (Test-Path -LiteralPath $RuntimePrefix -PathType Container)) {
  throw "Runtime prefix does not exist: $RuntimePrefix"
}

Write-Host "Locating signtool"
$signTool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
  Sort-Object FullName -Descending |
  Select-Object -First 1
if (-not $signTool) {
  throw "signtool.exe was not found"
}

$subject = "CN=conda-ship CI $([guid]::NewGuid())"
Write-Host "Creating code-signing certificate"
$certificate = New-SelfSignedCertificate `
  -Type CodeSigningCert `
  -Subject $subject `
  -CertStoreLocation "Cert:\CurrentUser\My" `
  -HashAlgorithm SHA256 `
  -KeyExportPolicy Exportable
try {
  foreach ($name in @("demo", "demoz")) {
    $source = Join-Path $DistDirectory "${name}-${Target}.exe"
    $signed = Join-Path $TemporaryDirectory "${name}-${Target}-signed.exe"
    Write-Host "Inspecting runtime layout: $source"
    Copy-Item $source $signed
    $unsignedLength = (Get-Item $signed).Length
    if ($unsignedLength % 8 -ne 0) {
      throw "Stamped PE runtime is not 8-byte aligned"
    }

    $bytes = [System.IO.File]::ReadAllBytes($signed)
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    $sectionCount = [BitConverter]::ToUInt16($bytes, $peOffset + 6)
    $optionalSize = [BitConverter]::ToUInt16($bytes, $peOffset + 20)
    $sectionOffset = $peOffset + 24 + $optionalSize
    $lastSectionEnd = 0L
    $anchorCount = 0
    $anchorOffset = 0L
    $anchorRawSize = 0L
    for ($index = 0; $index -lt $sectionCount; $index++) {
      $entry = $sectionOffset + 40 * $index
      $sectionName = [Text.Encoding]::ASCII.GetString(
        (Get-ByteRange $bytes $entry 8)
      ).TrimEnd([char]0)
      $virtualSize = [BitConverter]::ToUInt32($bytes, $entry + 8)
      $rawSize = [BitConverter]::ToUInt32($bytes, $entry + 16)
      $rawOffset = [BitConverter]::ToUInt32($bytes, $entry + 20)
      $lastSectionEnd = [Math]::Max($lastSectionEnd, [long]$rawOffset + $rawSize)
      if ($sectionName -eq ".cship") {
        $anchorCount += 1
        $anchorOffset = $rawOffset
        $anchorRawSize = $rawSize
        $characteristics = [BitConverter]::ToUInt32($bytes, $entry + 36)
        if ($virtualSize -ne 100 -or $characteristics -ne 0x40000040) {
          throw "The .cship section does not have its canonical layout"
        }
      }
    }
    if ($anchorCount -ne 1) {
      throw "Expected exactly one .cship section"
    }
    $anchorEnd = $anchorOffset + $anchorRawSize
    if ($anchorEnd -ne $lastSectionEnd) {
      throw "The .cship section is not the final raw PE section"
    }
    $footerMagic = [Text.Encoding]::ASCII.GetString(
      (Get-ByteRange $bytes ($anchorOffset + 84) 16)
    )
    if ($footerMagic -ne "CONDA_SHIP_V0001") {
      throw "The .cship section does not contain a runtime footer"
    }
    for ($offset = $anchorOffset + 100; $offset -lt $anchorEnd; $offset++) {
      if ($bytes[$offset] -ne 0) {
        throw "The .cship section padding is not zero"
      }
    }

    $headerLength = [BitConverter]::ToUInt64($bytes, [int]$anchorOffset)
    $bundleLength = [BitConverter]::ToUInt64($bytes, [int]$anchorOffset + 8)
    $contentLength = [uint64]($headerLength + $bundleLength)
    $unalignedEnd = [uint64]($anchorEnd + $contentLength)
    $payloadPadding = [uint64]((8 - ($unalignedEnd % 8)) % 8)
    $payloadStart = [uint64]($anchorEnd + $payloadPadding)
    $payloadEnd = [uint64]($payloadStart + $contentLength)
    if ($payloadEnd -ne $unsignedLength) {
      throw "The PE runtime payload does not end at unsigned EOF"
    }
    for ($offset = $anchorEnd; $offset -lt $payloadStart; $offset++) {
      if ($bytes[$offset] -ne 0) {
        throw "The PE runtime alignment padding is not zero"
      }
    }

    $headerBytes = Get-ByteRange $bytes $payloadStart $headerLength
    $expectedHeaderHash = Get-ByteRange $bytes ($anchorOffset + 16) 32
    $actualHeaderHash = Get-Sha256 $headerBytes
    if (
      [Convert]::ToBase64String($actualHeaderHash) -ne
      [Convert]::ToBase64String($expectedHeaderHash)
    ) {
      throw "The PE runtime header does not match the signed anchor"
    }
    $bundleBytes = Get-ByteRange `
      $bytes `
      ($payloadStart + $headerLength) `
      $bundleLength
    $expectedBundleHash = Get-ByteRange $bytes ($anchorOffset + 48) 32
    $actualBundleHash = Get-Sha256 $bundleBytes
    if (
      [Convert]::ToBase64String($actualBundleHash) -ne
      [Convert]::ToBase64String($expectedBundleHash)
    ) {
      throw "The PE runtime bundle does not match the signed anchor"
    }

    $attackInfo = Join-Path $TemporaryDirectory (
      "${name}-${Target}-attack.info.json"
    )
    $attackInfoPath = Join-Path $DistDirectory "${name}-${Target}.info.json"
    $attackInfoObject = Get-Content $attackInfoPath -Raw | ConvertFrom-Json
    $attackUpdate = [pscustomobject]@{
      channel = "https://packages.example.test/runtime"
      package = "demo-runtime"
      "build-number" = 0
    }
    $attackInfoObject.update = $attackUpdate
    [System.IO.File]::WriteAllText(
      $attackInfo,
      ($attackInfoObject | ConvertTo-Json -Compress -Depth 20),
      [Text.UTF8Encoding]::new($false)
    )

    Write-Host "Signing runtime: $signed"
    & $signTool.FullName sign /fd SHA256 /sha1 $certificate.Thumbprint $signed
    if ($LASTEXITCODE -ne 0) {
      throw "signtool failed to sign $signed"
    }
    Write-Host "Checking signed runtime integrity: $signed"
    Assert-AuthenticodeIntegrity `
      $signed `
      $certificate.Thumbprint `
      "Signed runtime"
    Write-Host "Executing signed runtime: $signed"
    Assert-SignedRuntimeExecutes $signed $RuntimePrefix
    Write-Host "Signed runtime execution completed: $signed"

    $signedBytes = [System.IO.File]::ReadAllBytes($signed)
    $optionalOffset = $peOffset + 24
    $optionalMagic = [BitConverter]::ToUInt16($signedBytes, $optionalOffset)
    $dataDirectoryOffset = switch ($optionalMagic) {
      0x10b { $optionalOffset + 96 }
      0x20b { $optionalOffset + 112 }
      default { throw "Unexpected PE optional-header magic" }
    }
    $securityOffset = $dataDirectoryOffset + 8 * 4
    $certificateOffset = [BitConverter]::ToUInt32($signedBytes, $securityOffset)
    $certificateSize = [BitConverter]::ToUInt32($signedBytes, $securityOffset + 4)
    if ($certificateOffset -ne $payloadEnd) {
      throw "signtool did not place the certificate table at the runtime payload end"
    }
    if ($certificateOffset + $certificateSize -ne $signedBytes.Length) {
      throw "PE certificate table is not the final file data"
    }

    $anchorTampered = Join-Path $TemporaryDirectory (
      "${name}-${Target}-tampered-anchor.exe"
    )
    Copy-Item $signed $anchorTampered
    Flip-FileByte $anchorTampered ($anchorOffset + 16)
    Write-Host "Verifying modified anchor rejection: $anchorTampered"
    Assert-AuthenticodeHashMismatch $anchorTampered "Modified .cship anchor"
    Assert-RuntimeReadFailure `
      $anchorTampered `
      $attackInfo `
      "runtime data header checksum mismatch" `
      "Modified .cship anchor"

    $payloadTampered = Join-Path $TemporaryDirectory (
      "${name}-${Target}-tampered-payload.exe"
    )
    Copy-Item $signed $payloadTampered
    Flip-FileByte $payloadTampered $payloadStart
    Write-Host "Verifying modified payload behavior: $payloadTampered"
    $payloadSignature = Get-AuthenticodeSignature -LiteralPath $payloadTampered
    Write-Host "Overlay mutation Authenticode status: $($payloadSignature.Status)"
    Assert-RuntimeReadFailure `
      $payloadTampered `
      $attackInfo `
      "runtime data header checksum mismatch" `
      "Modified PE overlay payload"

    $movedCertificate = Join-Path $TemporaryDirectory (
      "${name}-${Target}-moved-certificate.exe"
    )
    $movedBytes = [byte[]]::new($signedBytes.Length + 8)
    [Buffer]::BlockCopy(
      $signedBytes,
      0,
      $movedBytes,
      0,
      [int]$certificateOffset
    )
    [Buffer]::BlockCopy(
      $signedBytes,
      [int]$certificateOffset,
      $movedBytes,
      [int]$certificateOffset + 8,
      [int]$certificateSize
    )
    [BitConverter]::GetBytes([uint32]($certificateOffset + 8)).CopyTo(
      $movedBytes,
      $securityOffset
    )
    [System.IO.File]::WriteAllBytes($movedCertificate, $movedBytes)
    Write-Host "Verifying moved certificate rejection: $movedCertificate"
    $movedSignature = Get-AuthenticodeSignature -LiteralPath $movedCertificate
    Write-Host "Moved certificate Authenticode status: $($movedSignature.Status)"
    Assert-RuntimeReadFailure `
      $movedCertificate `
      $attackInfo `
      "does not end at the certificate table offset" `
      "Moved PE certificate table"

    if ($name -eq "demo") {
      if ($bundleLength -ne 0) {
        throw "The shadow test requires the online fixture"
      }
      $headerJson = [Text.Encoding]::UTF8.GetString($headerBytes)
      $forgedHeaderObject = $headerJson | ConvertFrom-Json
      $forgedHeaderObject | Add-Member `
        -NotePropertyName update `
        -NotePropertyValue $attackUpdate `
        -Force
      $forgedHeaderBytes = [Text.Encoding]::UTF8.GetBytes(
        ($forgedHeaderObject | ConvertTo-Json -Compress -Depth 20)
      )
      $forged = [byte[]]::new($forgedHeaderBytes.Length + 100)
      [Buffer]::BlockCopy(
        $forgedHeaderBytes,
        0,
        $forged,
        0,
        $forgedHeaderBytes.Length
      )
      $cursor = $forgedHeaderBytes.Length
      [BitConverter]::GetBytes([uint64]$forgedHeaderBytes.Length).CopyTo(
        $forged,
        $cursor
      )
      $cursor += 8
      [BitConverter]::GetBytes([uint64]0).CopyTo($forged, $cursor)
      $cursor += 8
      (Get-Sha256 $forgedHeaderBytes).CopyTo(
        $forged,
        $cursor
      )
      $cursor += 32
      (Get-Sha256 ([byte[]]::new(0))).CopyTo(
        $forged,
        $cursor
      )
      $cursor += 32
      [BitConverter]::GetBytes([uint32]1).CopyTo($forged, $cursor)
      $cursor += 4
      [Text.Encoding]::ASCII.GetBytes("CONDA_SHIP_V0001").CopyTo(
        $forged,
        $cursor
      )

      $extraSize = [int]($forged.Length + ((8 - ($forged.Length % 8)) % 8))
      $shadowBytes = [byte[]]::new($signedBytes.Length + $extraSize)
      [Buffer]::BlockCopy(
        $signedBytes,
        0,
        $shadowBytes,
        0,
        $signedBytes.Length
      )
      [Buffer]::BlockCopy(
        $forged,
        0,
        $shadowBytes,
        $signedBytes.Length,
        $forged.Length
      )
      $firstCertificateLength = [BitConverter]::ToUInt32(
        $shadowBytes,
        $certificateOffset
      )
      $alignedFirstCertificateLength = [uint32](
        $firstCertificateLength +
        ((8 - ($firstCertificateLength % 8)) % 8)
      )
      if ($alignedFirstCertificateLength -ne $certificateSize) {
        throw "Expected one signtool WIN_CERTIFICATE record"
      }
      $shadowCertificateSize = [uint32]($certificateSize + $extraSize)
      [BitConverter]::GetBytes($shadowCertificateSize).CopyTo(
        $shadowBytes,
        $securityOffset + 4
      )
      [BitConverter]::GetBytes($shadowCertificateSize).CopyTo(
        $shadowBytes,
        $certificateOffset
      )
      $shadow = Join-Path $TemporaryDirectory (
        "${name}-${Target}-certificate-shadow.exe"
      )
      [System.IO.File]::WriteAllBytes($shadow, $shadowBytes)

      Write-Host "Verifying certificate-padding fixture: $shadow"
      Assert-AuthenticodeIntegrity `
        $shadow `
        $certificate.Thumbprint `
        "Certificate-padding fixture"
      Assert-RuntimeReadFailure `
        $shadow `
        $attackInfo `
        "runtime executable update configuration does not match artifact info" `
        "Certificate-padding shadow footer"
    }
  }
}
finally {
  Remove-Item "Cert:\CurrentUser\My\$($certificate.Thumbprint)" `
    -Force -ErrorAction SilentlyContinue
}
