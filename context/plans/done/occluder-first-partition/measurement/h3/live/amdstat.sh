#!/bin/sh
# One AMD Radeon Pro 5300M PerformanceStatistics sample: util %, core clock, temp, VRAM in use / free.
ioreg -c AMDRadeonX6000_AMDNavi14GraphicsAccelerator -r -d1 | grep -m1 '"PerformanceStatistics" =' | grep -oE '"(Device Utilization %|Core Clock\(MHz\)|Temperature\(C\)|inUseVidMemoryBytes|vramFreeBytes)"=[0-9]+' | sed 's/"//g; s/Device Utilization %/util/; s/Core Clock(MHz)/mhz/; s/Temperature(C)/temp/' | tr '\n' ' '; echo
