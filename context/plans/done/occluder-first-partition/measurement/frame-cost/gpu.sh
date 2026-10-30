#!/bin/sh
ioreg -c IOAccelerator | grep -oE '"(inUseVidMemoryBytes|vramFreeBytes|Device Utilization %|Core Clock\(MHz\)|Temperature\(C\))"=[0-9]+' | tr '\n' ' ' | sed 's/"//g'; echo
