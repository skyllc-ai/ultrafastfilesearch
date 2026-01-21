#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Creates a complex directory tree for MFT testing.
    
.DESCRIPTION
    Creates files, directories, hard links, symbolic links, junction points,
    alternate data streams, and then deletes some items to test MFT parsing.
    
.PARAMETER TargetDrive
    The drive letter to create the test tree on (e.g., "E").
    
.EXAMPLE
    .\create_mft_test_tree.ps1 -TargetDrive E
#>

param(
    [Parameter(Mandatory=$true)]
    [string]$TargetDrive
)

$ErrorActionPreference = "Stop"
$root = "${TargetDrive}:\MFT_TEST"

Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  MFT Test Tree Generator" -ForegroundColor Cyan
Write-Host "  Target: $root" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan

# Clean up if exists
if (Test-Path $root) {
    Write-Host "`n[CLEANUP] Removing existing test tree..." -ForegroundColor Yellow
    Remove-Item -Path $root -Recurse -Force
}

# 1. Create directory structure
Write-Host "`n[1/8] Creating directory structure..." -ForegroundColor Green
$dirs = @(
    "$root",
    "$root\Documents",
    "$root\Documents\Reports",
    "$root\Documents\Reports\2024",
    "$root\Media",
    "$root\Media\Photos",
    "$root\Media\Videos",
    "$root\Code",
    "$root\Code\Project1",
    "$root\Code\Project2",
    "$root\Backup",
    "$root\ToDelete"
)
foreach ($dir in $dirs) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    Write-Host "  Created: $dir"
}

# 2. Create regular files
Write-Host "`n[2/8] Creating regular files..." -ForegroundColor Green
$files = @{
    "$root\readme.txt" = "This is the MFT test tree root."
    "$root\Documents\doc1.txt" = "Document 1 content"
    "$root\Documents\doc2.txt" = "Document 2 content"
    "$root\Documents\Reports\report.txt" = "Annual report"
    "$root\Documents\Reports\2024\q1.txt" = "Q1 2024 data"
    "$root\Documents\Reports\2024\q2.txt" = "Q2 2024 data"
    "$root\Media\Photos\photo1.jpg" = "FAKE_JPG_DATA_1234567890"
    "$root\Media\Photos\photo2.jpg" = "FAKE_JPG_DATA_0987654321"
    "$root\Media\Videos\video1.mp4" = "FAKE_MP4_DATA" * 100
    "$root\Code\Project1\main.rs" = "fn main() { println!(""Hello""); }"
    "$root\Code\Project1\lib.rs" = "pub fn greet() { }"
    "$root\Code\Project2\app.py" = "print('Hello')"
    "$root\Backup\backup1.bak" = "Backup data 1"
    "$root\ToDelete\temp1.tmp" = "Temporary file 1"
    "$root\ToDelete\temp2.tmp" = "Temporary file 2"
    "$root\ToDelete\temp3.tmp" = "Temporary file 3"
}
foreach ($file in $files.Keys) {
    Set-Content -Path $file -Value $files[$file]
    Write-Host "  Created: $file"
}

# 3. Create hard links (same file, multiple names)
Write-Host "`n[3/8] Creating hard links..." -ForegroundColor Green
$hardlinks = @(
    @{ Target = "$root\Documents\doc1.txt"; Link = "$root\Backup\doc1_hardlink.txt" },
    @{ Target = "$root\Code\Project1\main.rs"; Link = "$root\Code\Project2\main_shared.rs" },
    @{ Target = "$root\Media\Photos\photo1.jpg"; Link = "$root\Documents\photo1_link.jpg" }
)
foreach ($hl in $hardlinks) {
    New-Item -ItemType HardLink -Path $hl.Link -Target $hl.Target | Out-Null
    Write-Host "  Hardlink: $($hl.Link) -> $($hl.Target)"
}

# 4. Create symbolic links (files)
Write-Host "`n[4/8] Creating symbolic links (files)..." -ForegroundColor Green
$symlinks = @(
    @{ Target = "$root\readme.txt"; Link = "$root\Documents\readme_symlink.txt" },
    @{ Target = "$root\Code\Project1\lib.rs"; Link = "$root\Code\lib_symlink.rs" }
)
foreach ($sl in $symlinks) {
    New-Item -ItemType SymbolicLink -Path $sl.Link -Target $sl.Target | Out-Null
    Write-Host "  Symlink: $($sl.Link) -> $($sl.Target)"
}

# 5. Create junction points (directory links)
Write-Host "`n[5/8] Creating junction points..." -ForegroundColor Green
$junctions = @(
    @{ Target = "$root\Documents\Reports"; Link = "$root\ReportsJunction" },
    @{ Target = "$root\Media\Photos"; Link = "$root\PhotosJunction" }
)
foreach ($jp in $junctions) {
    New-Item -ItemType Junction -Path $jp.Link -Target $jp.Target | Out-Null
    Write-Host "  Junction: $($jp.Link) -> $($jp.Target)"
}

# 6. Create Alternate Data Streams (ADS)
Write-Host "`n[6/8] Creating Alternate Data Streams..." -ForegroundColor Green
$ads = @(
    @{ File = "$root\readme.txt"; Stream = "metadata"; Content = "Author: TestUser" },
    @{ File = "$root\readme.txt"; Stream = "Zone.Identifier"; Content = "[ZoneTransfer]`nZoneId=3" },
    @{ File = "$root\Documents\doc1.txt"; Stream = "comments"; Content = "Review needed" },
    @{ File = "$root\Media\Photos\photo1.jpg"; Stream = "com.dropbox.attrs"; Content = "dropbox_id=12345" },
    @{ File = "$root\Code\Project1\main.rs"; Stream = "build_info"; Content = "rustc 1.75.0" }
)
foreach ($a in $ads) {
    Set-Content -Path "$($a.File):$($a.Stream)" -Value $a.Content
    Write-Host "  ADS: $($a.File):$($a.Stream)"
}

# 7. Set various attributes
Write-Host "`n[7/8] Setting file attributes..." -ForegroundColor Green
Set-ItemProperty -Path "$root\Backup\backup1.bak" -Name Attributes -Value "Hidden"
Set-ItemProperty -Path "$root\Documents\doc2.txt" -Name Attributes -Value "ReadOnly"
Set-ItemProperty -Path "$root\Media\Videos" -Name Attributes -Value "Hidden,System"
Write-Host "  Hidden: $root\Backup\backup1.bak"
Write-Host "  ReadOnly: $root\Documents\doc2.txt"  
Write-Host "  Hidden+System: $root\Media\Videos"

# 8. Delete some items (to create "not in use" MFT records)
Write-Host "`n[8/8] Deleting items (creates unused MFT records)..." -ForegroundColor Green
Remove-Item -Path "$root\ToDelete\temp1.tmp" -Force
Remove-Item -Path "$root\ToDelete\temp2.tmp" -Force
Remove-Item -Path "$root\ToDelete" -Recurse -Force
Write-Host "  Deleted: $root\ToDelete\temp1.tmp"
Write-Host "  Deleted: $root\ToDelete\temp2.tmp"
Write-Host "  Deleted: $root\ToDelete (directory)"

# Summary
Write-Host "`n═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  TEST TREE CREATED SUCCESSFULLY" -ForegroundColor Green
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host @"

Summary:
  - Directories: $($dirs.Count)
  - Regular files: $($files.Count)
  - Hard links: $($hardlinks.Count)
  - Symbolic links: $($symlinks.Count)
  - Junction points: $($junctions.Count)
  - Alternate Data Streams: $($ads.Count)
  - Deleted items: 3

Expected MFT entries (approx):
  Base files/dirs: ~$($dirs.Count + $files.Count - 3)
  + Hard links: +$($hardlinks.Count) (same FRS, different names)
  + ADS: +$($ads.Count) (separate stream entries in C++)
  + "." entries: +$($dirs.Count - 3) (directory self-refs in C++)

To test:
  1. Run C++: UltraFastFileSearch.exe --drive $TargetDrive > cpp_test.csv
  2. Run Rust: uffs_mft.exe read --drive $TargetDrive > rust_test.csv
  3. Compare the outputs

"@ -ForegroundColor White

