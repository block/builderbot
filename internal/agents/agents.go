package agents

import (
	"os/exec"
	"strconv"
	"strings"
	"sync"
)

// Info represents a running Claude agent.
type Info struct {
	PID    int
	Prompt string
}

// FindActive returns a map from working directory path to active Claude agents.
func FindActive() map[string][]Info {
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

	var mu sync.Mutex
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
					mu.Lock()
					result[dir] = append(result[dir], Info{PID: p.pid, Prompt: p.prompt})
					mu.Unlock()
					break
				}
			}
		}(p)
	}
	wg.Wait()

	return result
}
