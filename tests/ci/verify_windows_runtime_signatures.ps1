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
  $output = & $CsExecutable package-update `
    --info $Info `
    --binary $Binary `
    --out-dir $packageDir 2>&1 | Out-String
  $status = $LASTEXITCODE
  if ($status -eq 0 -or $output -notmatch [regex]::Escape($Expected)) {
    throw "$Label was not rejected as expected: $output"
  }
}

$signTool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
  Sort-Object FullName -Descending |
  Select-Object -First 1
if (-not $signTool) {
  throw "signtool.exe was not found"
}

$subject = "CN=conda-ship CI $([guid]::NewGuid())"
$certificate = New-SelfSignedCertificate `
  -Type CodeSigningCert `
  -Subject $subject `
  -CertStoreLocation "Cert:\CurrentUser\My" `
  -HashAlgorithm SHA256 `
  -KeyExportPolicy Exportable
$trustedCertificate = $null
try {
  $trustedCertificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
    $certificate.RawData
  )
  $rootStore = [System.Security.Cryptography.X509Certificates.X509Store]::new(
    "Root",
    [System.Security.Cryptography.X509Certificates.StoreLocation]::CurrentUser
  )
  try {
    $rootStore.Open(
      [System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite
    )
    $rootStore.Add($trustedCertificate)
  }
  finally {
    $rootStore.Close()
  }

  foreach ($name in @("demo", "demoz")) {
    $source = Join-Path $DistDirectory "${name}-${Target}.exe"
    $signed = Join-Path $TemporaryDirectory "${name}-${Target}-signed.exe"
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
    $actualHeaderHash = [Security.Cryptography.SHA256]::HashData($headerBytes)
    if (
      [Convert]::ToHexString($actualHeaderHash) -ne
      [Convert]::ToHexString($expectedHeaderHash)
    ) {
      throw "The PE runtime header does not match the signed anchor"
    }
    $bundleBytes = Get-ByteRange `
      $bytes `
      ($payloadStart + $headerLength) `
      $bundleLength
    $expectedBundleHash = Get-ByteRange $bytes ($anchorOffset + 48) 32
    $actualBundleHash = [Security.Cryptography.SHA256]::HashData($bundleBytes)
    if (
      [Convert]::ToHexString($actualBundleHash) -ne
      [Convert]::ToHexString($expectedBundleHash)
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

    & $signTool.FullName sign /fd SHA256 /sha1 $certificate.Thumbprint $signed
    if ($LASTEXITCODE -ne 0) {
      throw "signtool failed to sign $signed"
    }
    & $signTool.FullName verify /pa /v $signed
    if ($LASTEXITCODE -ne 0) {
      throw "signtool failed to verify $signed"
    }
    $null = & $signed --help
    if ($LASTEXITCODE -ne 0) {
      throw "The signed runtime did not execute"
    }

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
    $null = & $signTool.FullName verify /pa $anchorTampered 2>&1
    if ($LASTEXITCODE -eq 0) {
      throw "Authenticode accepted a modified .cship anchor"
    }
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
    $null = & $signTool.FullName verify /pa $payloadTampered 2>&1
    Write-Host "Overlay mutation SignTool status: $LASTEXITCODE"
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
    $null = & $signTool.FullName verify /pa $movedCertificate 2>&1
    Write-Host "Moved certificate SignTool status: $LASTEXITCODE"
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
      $forgedHeaderObject.update = $attackUpdate
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
      [Security.Cryptography.SHA256]::HashData($forgedHeaderBytes).CopyTo(
        $forged,
        $cursor
      )
      $cursor += 32
      [Security.Cryptography.SHA256]::HashData([byte[]]::new(0)).CopyTo(
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

      & $signTool.FullName verify /pa /v $shadow
      if ($LASTEXITCODE -ne 0) {
        throw "signtool rejected the certificate-padding shadow fixture"
      }
      Assert-RuntimeReadFailure `
        $shadow `
        $attackInfo `
        "runtime executable update configuration does not match artifact info" `
        "Certificate-padding shadow footer"
    }
  }
}
finally {
  if ($trustedCertificate) {
    $trustedCertificate.Dispose()
  }
  Remove-Item "Cert:\CurrentUser\My\$($certificate.Thumbprint)" `
    -Force -ErrorAction SilentlyContinue
  Remove-Item "Cert:\CurrentUser\Root\$($certificate.Thumbprint)" `
    -Force -ErrorAction SilentlyContinue
}
