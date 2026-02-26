package agents

import (
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Info represents a running Claude agent.
type Info struct {
	PID    int
	Prompt string
}

var (
	mu     sync.RWMutex
	cached map[string][]Info

	pollMu sync.Mutex
	stopCh chan struct{}
)

// StartPolling begins background polling for active agents every 5 seconds.
func StartPolling() {
	pollMu.Lock()
	defer pollMu.Unlock()

	if stopCh != nil {
		close(stopCh)
	}
	stopCh = make(chan struct{})
	ch := stopCh
	// Do an initial poll synchronously so the first read has data.
	result := poll()
	mu.Lock()
	cached = result
	mu.Unlock()

	go func() {
		ticker := time.NewTicker(5 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ch:
				return
			case <-ticker.C:
				result := poll()
				mu.Lock()
				cached = result
				mu.Unlock()
			}
		}
	}()
}

// StopPolling stops the background polling goroutine.
func StopPolling() {
	pollMu.Lock()
	defer pollMu.Unlock()

	if stopCh != nil {
		close(stopCh)
		stopCh = nil
	}
}

// FindActive returns the latest snapshot of active Claude agents.
// Never blocks on process inspection; returns cached data from background poll.
func FindActive() map[string][]Info {
	mu.RLock()
	defer mu.RUnlock()
	return cached
}

func poll() map[string][]Info {
	out, err := exec.Command("ps", "-eo", "pid,args").Output()
	if err != nil {
		return nil
	}

	type proc struct {
		pid    int
		prompt string
	}

	var procs []proc
	for _, line := range strings.Split(string(out), "\n") {
		line = strings.TrimSpace(line)
		parts := strings.SplitN(line, " ", 2)
		if len(parts) < 2 {
			continue
		}

		pid, err := strconv.Atoi(parts[0])
		if err != nil {
			continue
		}

		args := parts[1]
		fields := strings.Fields(args)
		if len(fields) == 0 || !strings.HasSuffix(fields[0], "/claude") {
			continue
		}

		prompt := ""
		if idx := strings.Index(args, " -- "); idx >= 0 {
			prompt = args[idx+4:]
		}

		procs = append(procs, proc{pid: pid, prompt: prompt})
	}

	var lmu sync.Mutex
	var wg sync.WaitGroup
	result := make(map[string][]Info)

	for _, p := range procs {
		wg.Add(1)
		go func(p proc) {
			defer wg.Done()
			out, err := exec.Command("lsof", "-a", "-p", strconv.Itoa(p.pid), "-d", "cwd", "-Fn").Output()
			if err != nil {
				return
			}
			for _, line := range strings.Split(string(out), "\n") {
				if strings.HasPrefix(line, "n/") {
					dir := line[1:]
					lmu.Lock()
					result[dir] = append(result[dir], Info{PID: p.pid, Prompt: p.prompt})
					lmu.Unlock()
					break
				}
			}
		}(p)
	}
	wg.Wait()

	return result
}
