package otel

import (
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"regexp"
	"strings"
	"syscall"
	"testing"
	"time"
)

// ─── Status visibility ──────────────────────────────────────────────────────
// Verifies that `tukituki status` and `tukituki status --json` expose the
// OTel receiver port on the otel-errors row after the collector has started.

func TestExamples_StatusShowsOtelPort(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping examples integration test in short mode")
	}

	root := projectRoot(t)
	examplesDir := root + "examples"
	if _, err := os.Stat(examplesDir + "/.run"); err != nil {
		t.Fatalf("examples/.run not found: %v", err)
	}

	binPath := buildTukituki(t, root)
	stateDir := t.TempDir() + "/.tukituki"

	cleanPreviousRun(examplesDir, binPath)
	defer func() {
		_ = runCmd(examplesDir, binPath, "stop", "--state-dir", stateDir).Run()
	}()

	startOut, err := runCmd(examplesDir, binPath, "start", "--state-dir", stateDir).CombinedOutput()
	if err != nil {
		t.Fatalf("tukituki start: %v\n%s", err, startOut)
	}

	// Wait for services to come up so otel-errors is guaranteed running.
	waitForPortTimeout(t, 8081, 30*time.Second)
	waitForPortTimeout(t, 8082, 30*time.Second)
	waitForPortTimeout(t, 8083, 30*time.Second)

	// Text: the otel-errors row should include the "listening on :<port>" suffix.
	textOut, err := runCmd(examplesDir, binPath, "status", "--state-dir", stateDir).CombinedOutput()
	if err != nil {
		t.Fatalf("tukituki status: %v\n%s", err, textOut)
	}
	text := string(textOut)
	t.Logf("status text:\n%s", text)

	portFromText := extractListeningPort(text)
	if portFromText == 0 {
		t.Fatalf("status text did not contain 'listening on :<port>' for otel-errors:\n%s", text)
	}

	// JSON: the otel-errors entry should include a populated Address field.
	jsonOut, err := runCmd(examplesDir, binPath, "status", "--state-dir", stateDir, "--json").CombinedOutput()
	if err != nil {
		t.Fatalf("tukituki status --json: %v\n%s", err, jsonOut)
	}
	t.Logf("status json:\n%s", jsonOut)

	var entries []struct {
		Name    string `json:"name"`
		Status  string `json:"status"`
		Address string `json:"address"`
	}
	if err := json.Unmarshal(jsonOut, &entries); err != nil {
		t.Fatalf("parse status json: %v\n%s", err, jsonOut)
	}
	var otelEntry *struct {
		Name    string `json:"name"`
		Status  string `json:"status"`
		Address string `json:"address"`
	}
	for i := range entries {
		if entries[i].Name == "otel-errors" {
			otelEntry = &entries[i]
			break
		}
	}
	if otelEntry == nil {
		t.Fatalf("status json did not include otel-errors entry: %s", jsonOut)
	}
	if otelEntry.Status != "running" {
		t.Errorf("otel-errors status = %q, want running", otelEntry.Status)
	}
	if otelEntry.Address == "" {
		t.Errorf("otel-errors address is empty, want 127.0.0.1:<port>")
	}
	wantAddr := fmt.Sprintf("127.0.0.1:%d", portFromText)
	if otelEntry.Address != wantAddr {
		t.Errorf("otel-errors address = %q, want %q", otelEntry.Address, wantAddr)
	}
}

// ─── Port recovery ──────────────────────────────────────────────────────────
// Simulates the "tukituki was down, something else grabbed the saved port"
// scenario: start the examples, stop just the collector, steal its port,
// start tukituki again. The collector must pick a new port, and the
// otel:true services must be restarted so their OTEL_EXPORTER_OTLP_ENDPOINT
// points at the new collector.

func TestExamples_OtelPortRecovery(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping examples integration test in short mode")
	}

	root := projectRoot(t)
	examplesDir := root + "examples"
	if _, err := os.Stat(examplesDir + "/.run"); err != nil {
		t.Fatalf("examples/.run not found: %v", err)
	}

	binPath := buildTukituki(t, root)
	stateDir := t.TempDir() + "/.tukituki"

	cleanPreviousRun(examplesDir, binPath)
	defer func() {
		_ = runCmd(examplesDir, binPath, "stop", "--state-dir", stateDir).Run()
	}()

	if out, err := runCmd(examplesDir, binPath, "start", "--state-dir", stateDir).CombinedOutput(); err != nil {
		t.Fatalf("tukituki start (initial): %v\n%s", err, out)
	}

	waitForPortTimeout(t, 8081, 30*time.Second)
	waitForPortTimeout(t, 8082, 30*time.Second)
	waitForPortTimeout(t, 8083, 30*time.Second)

	// Record the first port, then confirm the first round of errors reaches the collector.
	initialPort := queryOtelPort(t, examplesDir, binPath, stateDir)
	if initialPort == 0 {
		t.Fatalf("initial otel port is 0")
	}
	t.Logf("initial otel port: %d", initialPort)

	triggerErrors(t)
	time.Sleep(3 * time.Second)

	logPath := stateDir + "/logs/otel-errors.log"
	initialLog := readFile(t, logPath)
	expectedErrors := []string{
		"[go-api] database connection refused",
		"[go-worker] redis timeout: connection pool exhausted",
		"[python-web] upstream service unavailable",
	}
	for _, want := range expectedErrors {
		if !strings.Contains(initialLog, want) {
			dumpServiceLogs(t, stateDir)
			t.Fatalf("initial otel-errors.log missing %q:\n%s", want, initialLog)
		}
	}

	// Stop just the collector. The saved otel-port file is left intact so
	// the next start will try to reuse it.
	if out, err := runCmd(examplesDir, binPath, "stop", "otel-errors", "--state-dir", stateDir).CombinedOutput(); err != nil {
		t.Fatalf("tukituki stop otel-errors: %v\n%s", err, out)
	}
	// Give the OS a moment to release the port before we claim it.
	waitForPortFreed(t, initialPort, 3*time.Second)

	// Claim the old port so the next EnsureOtelCollector call detects it as
	// unbindable and must pick a new one.
	lis, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", initialPort))
	if err != nil {
		t.Fatalf("claim old otel port %d: %v", initialPort, err)
	}
	defer lis.Close()

	// Second start: should detect the stale port, pick a new one, and
	// restart the otel:true services so they re-read the endpoint env.
	startOut, err := runCmd(examplesDir, binPath, "start", "--state-dir", stateDir).CombinedOutput()
	if err != nil {
		t.Fatalf("tukituki start (recovery): %v\n%s", err, startOut)
	}
	t.Logf("recovery start output:\n%s", startOut)
	if !strings.Contains(string(startOut), "is no longer available") {
		t.Errorf("expected warning about stale port in start output:\n%s", startOut)
	}

	newPort := queryOtelPort(t, examplesDir, binPath, stateDir)
	if newPort == 0 {
		t.Fatalf("new otel port is 0")
	}
	if newPort == initialPort {
		t.Fatalf("new otel port %d == initial %d; expected a different port", newPort, initialPort)
	}
	t.Logf("new otel port: %d (was %d)", newPort, initialPort)

	// Release the stolen port and wait for the restarted services to come back.
	lis.Close()

	waitForPortTimeout(t, 8081, 60*time.Second)
	waitForPortTimeout(t, 8082, 60*time.Second)
	waitForPortTimeout(t, 8083, 60*time.Second)

	// Trigger a new round of errors. These must reach the collector on the
	// NEW port — which only works if the services were restarted and picked
	// up the new OTEL_EXPORTER_OTLP_ENDPOINT.
	triggerErrors(t)
	time.Sleep(5 * time.Second)

	// The collector truncates its log on (re)start, so we check for expected
	// error markers in the current log rather than size growth. All three
	// services producing errors proves they reached the new collector port.
	finalLog := readFile(t, logPath)
	for _, want := range expectedErrors {
		if !strings.Contains(finalLog, want) {
			dumpServiceLogs(t, stateDir)
			t.Fatalf("post-recovery otel-errors.log missing %q:\n%s", want, finalLog)
		}
	}
}

// ─── Upgrade scenario ──────────────────────────────────────────────────────
// Simulates upgrading tukituki in a project whose existing state predates
// the saved-port mechanism: otel:true services are running, but there is
// no .tukituki/otel-port file. The children's OTEL_EXPORTER_OTLP_ENDPOINT
// points at whatever random port an older tukituki picked, which no longer
// matches anything. EnsureOtelCollector must restart them even though
// savedPort == 0.

func TestExamples_OtelUpgradeNoSavedPort(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping examples integration test in short mode")
	}

	root := projectRoot(t)
	examplesDir := root + "examples"
	if _, err := os.Stat(examplesDir + "/.run"); err != nil {
		t.Fatalf("examples/.run not found: %v", err)
	}

	binPath := buildTukituki(t, root)
	stateDir := t.TempDir() + "/.tukituki"

	cleanPreviousRun(examplesDir, binPath)
	defer func() {
		_ = runCmd(examplesDir, binPath, "stop", "--state-dir", stateDir).Run()
	}()

	if out, err := runCmd(examplesDir, binPath, "start", "--state-dir", stateDir).CombinedOutput(); err != nil {
		t.Fatalf("tukituki start (initial): %v\n%s", err, out)
	}
	waitForPortTimeout(t, 8081, 30*time.Second)
	waitForPortTimeout(t, 8082, 30*time.Second)
	waitForPortTimeout(t, 8083, 30*time.Second)

	// Simulate pre-upgrade state: stop the collector, delete the saved-port
	// file (children remain alive with stale env pointing at the old port).
	if out, err := runCmd(examplesDir, binPath, "stop", "otel-errors", "--state-dir", stateDir).CombinedOutput(); err != nil {
		t.Fatalf("tukituki stop otel-errors: %v\n%s", err, out)
	}
	if err := os.Remove(stateDir + "/otel-port"); err != nil && !os.IsNotExist(err) {
		t.Fatalf("remove otel-port: %v", err)
	}

	// Next start: savedPort is 0, so the port recovery path must still
	// restart running otel:true children to refresh their env.
	startOut, err := runCmd(examplesDir, binPath, "start", "--state-dir", stateDir).CombinedOutput()
	if err != nil {
		t.Fatalf("tukituki start (upgrade): %v\n%s", err, startOut)
	}
	t.Logf("upgrade start output:\n%s", startOut)
	for _, name := range []string{"go-api", "go-worker", "python-web"} {
		want := fmt.Sprintf("restarting %s to pick up new collector endpoint", name)
		if !strings.Contains(string(startOut), want) {
			t.Errorf("upgrade start output missing %q:\n%s", want, startOut)
		}
	}

	waitForPortTimeout(t, 8081, 60*time.Second)
	waitForPortTimeout(t, 8082, 60*time.Second)
	waitForPortTimeout(t, 8083, 60*time.Second)

	triggerErrors(t)
	time.Sleep(5 * time.Second)

	logPath := stateDir + "/logs/otel-errors.log"
	log := readFile(t, logPath)
	expectedErrors := []string{
		"[go-api] database connection refused",
		"[go-worker] redis timeout: connection pool exhausted",
		"[python-web] upstream service unavailable",
	}
	for _, want := range expectedErrors {
		if !strings.Contains(log, want) {
			dumpServiceLogs(t, stateDir)
			t.Fatalf("upgrade otel-errors.log missing %q:\n%s", want, log)
		}
	}
}

// ─── Restart leaves no orphans ──────────────────────────────────────────────
// Exercises the real `go run` flow: start examples, record each service's
// shell leader PID, restart them, and verify the old process groups are
// fully reaped. This protects against the "go run supervisor orphaned under
// init" regression where Stop returned while descendants were still alive
// and left ppid=1 stragglers behind.

func TestExamples_RestartLeavesNoOrphans(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping examples integration test in short mode")
	}

	root := projectRoot(t)
	examplesDir := root + "examples"
	if _, err := os.Stat(examplesDir + "/.run"); err != nil {
		t.Fatalf("examples/.run not found: %v", err)
	}

	binPath := buildTukituki(t, root)
	stateDir := t.TempDir() + "/.tukituki"

	cleanPreviousRun(examplesDir, binPath)
	defer func() {
		_ = runCmd(examplesDir, binPath, "stop", "--state-dir", stateDir).Run()
	}()

	if out, err := runCmd(examplesDir, binPath, "start", "--state-dir", stateDir).CombinedOutput(); err != nil {
		t.Fatalf("tukituki start: %v\n%s", err, out)
	}
	waitForPortTimeout(t, 8081, 30*time.Second)
	waitForPortTimeout(t, 8082, 30*time.Second)
	waitForPortTimeout(t, 8083, 30*time.Second)

	services := []string{"go-api", "go-worker", "python-web"}
	oldLeaders := readLeaderPIDs(t, stateDir, services)

	for _, svc := range services {
		if out, err := runCmd(examplesDir, binPath, "restart", svc, "--state-dir", stateDir).CombinedOutput(); err != nil {
			t.Fatalf("tukituki restart %s: %v\n%s", svc, err, out)
		}
	}

	// After each service's restart the OLD process group must be empty.
	// If it isn't, the leader (shell) may have died while descendants
	// (`go run`, the compiled binary) were still around — exactly the
	// orphan-accumulation case the user hit.
	for _, svc := range services {
		oldPID := oldLeaders[svc]
		if oldPID <= 0 {
			t.Errorf("no prior leader PID recorded for %s", svc)
			continue
		}
		// Give stragglers a brief grace period before failing.
		deadline := time.Now().Add(3 * time.Second)
		for time.Now().Before(deadline) && testGroupAlive(oldPID) {
			time.Sleep(100 * time.Millisecond)
		}
		if testGroupAlive(oldPID) {
			_ = syscall.Kill(-oldPID, syscall.SIGKILL)
			t.Errorf("old process group for %s (leader=%d) still has members after restart — orphans leaked", svc, oldPID)
		}
	}

	// And the new services should actually be serving again.
	waitForPortTimeout(t, 8081, 30*time.Second)
	waitForPortTimeout(t, 8082, 30*time.Second)
	waitForPortTimeout(t, 8083, 30*time.Second)
}

// readLeaderPIDs returns the PID recorded in state.json for each named
// target.
func readLeaderPIDs(t *testing.T, stateDir string, names []string) map[string]int {
	t.Helper()
	data, err := os.ReadFile(stateDir + "/state.json")
	if err != nil {
		t.Fatalf("read state.json: %v", err)
	}
	var st struct {
		Processes map[string]struct {
			PID int `json:"pid"`
		} `json:"processes"`
	}
	if err := json.Unmarshal(data, &st); err != nil {
		t.Fatalf("parse state.json: %v\n%s", err, data)
	}
	out := make(map[string]int, len(names))
	for _, n := range names {
		if p, ok := st.Processes[n]; ok {
			out[n] = p.PID
		}
	}
	return out
}

// testGroupAlive mirrors groupAlive in the process package (which is
// unexported here) for the e2e test.
func testGroupAlive(leaderPID int) bool {
	if leaderPID <= 0 {
		return false
	}
	err := syscall.Kill(-leaderPID, 0)
	if err == nil {
		return true
	}
	if errors.Is(err, syscall.ESRCH) {
		return false
	}
	if errors.Is(err, syscall.EPERM) {
		return true
	}
	return false
}

// ─── Helpers ────────────────────────────────────────────────────────────────

func buildTukituki(t *testing.T, root string) string {
	t.Helper()
	binDir := t.TempDir()
	binPath := binDir + "/tukituki"
	if out, err := runCmd(root, "go", "build", "-o", binPath, "./cmd/tukituki/").CombinedOutput(); err != nil {
		t.Fatalf("build tukituki: %v\n%s", err, out)
	}
	return binPath
}

func cleanPreviousRun(examplesDir, binPath string) {
	_ = runCmd(examplesDir, binPath, "stop",
		"--state-dir", examplesDir+"/.tukituki",
	).Run()
	for _, port := range []int{8081, 8082, 8083} {
		killPort(port)
	}
	time.Sleep(500 * time.Millisecond)
}

func triggerErrors(t *testing.T) {
	t.Helper()
	for _, port := range []int{8081, 8082, 8083} {
		resp, err := http.Get(fmt.Sprintf("http://127.0.0.1:%d/", port))
		if err != nil {
			t.Fatalf("curl port %d: %v", port, err)
		}
		resp.Body.Close()
	}
}

func queryOtelPort(t *testing.T, examplesDir, binPath, stateDir string) int {
	t.Helper()
	out, err := runCmd(examplesDir, binPath, "status", "--state-dir", stateDir, "--json").CombinedOutput()
	if err != nil {
		t.Fatalf("tukituki status --json: %v\n%s", err, out)
	}
	var entries []struct {
		Name    string `json:"name"`
		Address string `json:"address"`
	}
	if err := json.Unmarshal(out, &entries); err != nil {
		t.Fatalf("parse status json: %v\n%s", err, out)
	}
	for _, e := range entries {
		if e.Name == "otel-errors" && e.Address != "" {
			_, portStr, _ := strings.Cut(e.Address, ":")
			var port int
			if _, err := fmt.Sscanf(portStr, "%d", &port); err == nil {
				return port
			}
		}
	}
	return 0
}

var listeningPortRE = regexp.MustCompile(`listening on :(\d+)`)

func extractListeningPort(text string) int {
	m := listeningPortRE.FindStringSubmatch(text)
	if len(m) != 2 {
		return 0
	}
	var port int
	if _, err := fmt.Sscanf(m[1], "%d", &port); err != nil {
		return 0
	}
	return port
}

func readFile(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return ""
		}
		t.Fatalf("read %s: %v", path, err)
	}
	return string(data)
}

func waitForPortFreed(t *testing.T, port int, timeout time.Duration) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		l, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", port))
		if err == nil {
			l.Close()
			return
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatalf("port %d was not freed within %s", port, timeout)
}

func dumpServiceLogs(t *testing.T, stateDir string) {
	t.Helper()
	for _, name := range []string{"go-api", "go-worker", "python-web", "otel-errors"} {
		data, _ := os.ReadFile(stateDir + "/logs/" + name + ".log")
		t.Logf("%s.log:\n%s", name, data)
	}
}
