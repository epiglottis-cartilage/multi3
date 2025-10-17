<#
.SYNOPSIS
Batch set static IP addresses in a continuous range for network adapters

.DESCRIPTION
This script allows users to input an IP address range, subnet mask, gateway, and DNS servers,
then configure all static IP addresses within the specified range for the selected network adapter in batches
#>

# Check for administrator privileges
#Requires -RunAsAdministrator

function Test-IPAddress {
    <# Verify if the IP address format is correct #>
    param([string]$IP)
    $ipRegex = "^((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$"
    return $IP -match $ipRegex
}

function Convert-IPToInt {
    param([string]$IP)
    $octets = $IP -split '\.'
    return [long]$octets[0] * 16777216 + [long]$octets[1] * 65536 + [long]$octets[2] * 256 + [long]$octets[3]
}

function Convert-IntToIP {
    param([long]$Int)
    $octet1 = [math]::Floor($Int / 16777216)
    $remaining = $Int % 16777216
    $octet2 = [math]::Floor($remaining / 65536)
    $remaining = $remaining % 65536
    $octet3 = [math]::Floor($remaining / 256)
    $octet4 = $remaining % 256
    return "$octet1.$octet2.$octet3.$octet4"
}

function Get-IPRange {
    param(
        [string]$StartIP,
        [string]$EndIP
    )
    
    if (-not (Test-IPAddress $StartIP) -or -not (Test-IPAddress $EndIP)) {
        Write-Error "Invalid IP address format"
        return $null
    }
    
    $startInt = Convert-IPToInt $StartIP
    $endInt = Convert-IPToInt $EndIP
    
    if ($startInt -gt $endInt) {
        Write-Error "Start IP is greater than End IP"
        return $null
    }
    
    $ipRange = @()
    for ($i = $startInt; $i -le $endInt; $i++) {
        $ipRange += Convert-IntToIP $i
    }
    
    return $ipRange
}

# Display welcome message
Write-Host "`n===== Batch Static IP Configuration Tool =====" -ForegroundColor Cyan

# Retrieve and display available network adapters
Write-Host "`nAvailable Network Adapters:" -ForegroundColor Yellow
$adapters = Get-NetAdapter | Where-Object { $_.Status -eq "Up" }
if ($adapters.Count -eq 0) {
    Write-Error "No active network adapters found."
    exit 1
}

# List adapters for user selection
for ($i = 0; $i -lt $adapters.Count; $i++) {
    Write-Host "$($i + 1). $($adapters[$i].InterfaceAlias) (Index: $($adapters[$i].InterfaceIndex))"
}

# Select adapter
do {
    $adapterChoice = Read-Host "`nPlease select an adapter (1-$($adapters.Count))"
    $adapterIndex = [int]$adapterChoice - 1
} while ($adapterIndex -lt 0 -or $adapterIndex -ge $adapters.Count)

$selectedAdapter = $adapters[$adapterIndex]
Write-Host "`nSelected adapter: $($selectedAdapter.InterfaceAlias)" -ForegroundColor Green

# Get IP configuration information
do {
    $startIP = Read-Host "Start IP address (e.g.: 10.103.35.100)"
} while (-not (Test-IPAddress $startIP))

do {
    $endIP = Read-Host "End IP address (e.g.: 10.103.35.200)"
} while (-not (Test-IPAddress $endIP))

# Generate IP range
$ipRange = Get-IPRange -StartIP $startIP -EndIP $endIP
if ($null -eq $ipRange) {
    exit 1
}

Write-Host "`nIP addresses to be configured: $($ipRange -join ', ')" -ForegroundColor Cyan

# Get subnet mask length
do {
    $prefixLength = Read-Host "Subnet mask length (e.g.: 24)"
} while (-not ($prefixLength -match '^\d+$') -or $prefixLength -lt 0 -or $prefixLength -gt 32)

$prefixLength = [int]$prefixLength

# Get gateway
do {
    $gateway = Read-Host "Gateway (optional, press Enter to skip)"
    if ($gateway -eq "") { break }
} while (-not (Test-IPAddress $gateway))

# Get DNS servers
do {
    $primaryDNS = Read-Host "Preferred DNS server (e.g.: 10.103.4.10)"
} while (-not (Test-IPAddress $primaryDNS))

do {
    $secondaryDNS = Read-Host "Alternative DNS server (e.g.: 10.103.4.45, optional, press Enter to skip)"
    if ($secondaryDNS -eq "") { break }
} while (-not (Test-IPAddress $secondaryDNS))

$dnsServers = @($primaryDNS)
if ($secondaryDNS -ne "") {
    $dnsServers += $secondaryDNS
}

# Confirm configuration
Write-Host "`n===== Configuration Check =====" -ForegroundColor Yellow
Write-Host "Adapter: $($selectedAdapter.InterfaceAlias)"
Write-Host "IP range: $startIP - $endIP"
Write-Host "Subnet mask length: $prefixLength"
Write-Host "Gateway: $(if ($gateway) { $gateway } else { 'None' })"
Write-Host "DNS servers: $($dnsServers -join ', ')"

$confirm = Read-Host "`nApply these settings? (Y/N)"
if ($confirm -ne 'Y' -and $confirm -ne 'y') {
    Write-Host "Operation cancelled" -ForegroundColor Red
    exit 0
}

# Clear existing IP configuration
try {
    Write-Host "`nClearing existing configuration..." -ForegroundColor Cyan
    Remove-NetIPAddress -InterfaceIndex $selectedAdapter.InterfaceIndex -AddressFamily IPv4 -Confirm:$false -ErrorAction SilentlyContinue
    Set-DnsClientServerAddress -InterfaceIndex $selectedAdapter.InterfaceIndex -ResetServerAddresses -ErrorAction SilentlyContinue
}
catch {
    Write-Warning "Warning: $_"
}

# Batch configure IP addresses
$successCount = 0
$failCount = 0

foreach ($ip in $ipRange) {
    try {
        Write-Host "`nConfiguring IP: $ip..." -ForegroundColor Cyan
        
        # Set IP address
        $params = @{
            InterfaceIndex = $selectedAdapter.InterfaceIndex
            IPAddress      = $ip
            PrefixLength   = $prefixLength
            AddressFamily  = 'IPv4'
            ErrorAction    = 'Stop'
        }
        
        # Set gateway only on the first IP if gateway is specified
        if ($gateway -and $ip -eq $ipRange[0]) {
            $params['DefaultGateway'] = $gateway
        }
        
        New-NetIPAddress @params | Out-Null
        
        # Set DNS servers (only need to set once)
        if ($ip -eq $ipRange[0]) {
            Set-DnsClientServerAddress -InterfaceIndex $selectedAdapter.InterfaceIndex -ServerAddresses $dnsServers -ErrorAction Stop
        }
        
        Write-Host "IP: $ip configured successfully" -ForegroundColor Green
        $successCount++
    }
    catch {
        Write-Error "Failed to configure IP: $ip : $_"
        $failCount++
    }
}

# Display result summary
Write-Host "`n===== Configuration Results =====" -ForegroundColor Yellow
Write-Host "Total IP addresses: $($ipRange.Count)"
Write-Host "Successfully configured: $successCount" -ForegroundColor Green
Write-Host "Failed to configure: $failCount" -ForegroundColor Red

Write-Host "`n===== Current DNS Configuration =====" -ForegroundColor Cyan
Get-DnsClientServerAddress -InterfaceIndex $selectedAdapter.InterfaceIndex -AddressFamily IPv4 | Select-Object -ExpandProperty ServerAddresses

# Display current configuration
Write-Host "`n===== Current IP Configuration =====" -ForegroundColor Cyan
Get-NetIPAddress -InterfaceIndex $selectedAdapter.InterfaceIndex -AddressFamily IPv4 | Format-Table IPAddress
