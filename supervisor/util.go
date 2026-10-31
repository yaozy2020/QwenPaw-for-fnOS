package main

import (
	"runtime"
)

func runtimeNumCPU() int { return runtime.NumCPU() }
