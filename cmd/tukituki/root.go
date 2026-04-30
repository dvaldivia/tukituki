// Copyright 2026 Daniel Valdivia
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"text/tabwriter"

	"github.com/dvaldivia/tukituki/internal/config"
	otelPkg "github.com/dvaldivia/tukituki/internal/otel"
	"github.com/dvaldivia/tukituki/internal/process"
	"github.com/dvaldivia/tukituki/internal/state"
	"github.com/dvaldivia/tukituki/internal/tui"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

// Version is set at build time via ldflags: -X main.Version=x.y.z
var Version = "dev"

var (
	cfgFile      string
	runDir       string
	stateDir     string
	jsonOutput   bool
	otelProtocol string
	otelSeverity string
	otelPort     int
)

// rootCmd is the base command; when called with no subcommand it starts the TUI.
var rootCmd = &cobra.Command{
	Use:   "tukituki",
	Short: "Manage multiple dev processes from a TUI or headless CLI",
	Long: `tukituki reads process definitions from .run/*.yaml and lets you
start, stop, restart, and tail their logs.

INTERACTIVE MODE (default, requires a terminal):
  Run with no arguments to open the interactive TUI. All processes are started
  automatically; the TUI lets you watch logs, restart, and stop them.

HEADLESS / SCRIPTED MODE (safe for automation and AI agents):
  Use subcommands for non-interactive control:
    tukituki list              - list configured targets
    tukituki status            - show runtime status of all targets
    tukituki start [name]      - start one or all targets
    tukituki stop  [name]      - stop  one or all targets
    tukituki restart <name>    - restart a target
    tukituki logs <name>       - tail logs (use --no-follow for one-shot read)

  Add --json to any subcommand for machine-readable JSON output.

CONFIGURATION:
  Process definitions live in .run/*.yaml (configurable via --run-dir or
  TUKITUKI_RUN_DIR). Runtime state is stored in .tukituki/ (configurable via
  --state-dir or TUKITUKI_STATE_DIR).`,
	RunE: runRoot,
}

// Execute is the entry point called from main.
func Execute() {
	if err := rootCmd.Execute(); err != nil {
		// cobra already prints the error; just exit non-zero.
		os.Exit(1)
	}
}

func init() {
	cobra.OnInitialize(initConfig)

	// Persistent flags — available to all subcommands.
	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "",
		"config file (default: .tukitukirc.yaml in cwd, then $HOME/.tukitukirc.yaml)")
	rootCmd.PersistentFlags().StringVar(&runDir, "run-dir", "",
		"directory containing YAML run definitions (env: TUKITUKI_RUN_DIR, default: .run)")
	rootCmd.PersistentFlags().StringVar(&stateDir, "state-dir", "",
		"directory for state file and logs (env: TUKITUKI_STATE_DIR, default: .tukituki)")
	rootCmd.PersistentFlags().BoolVar(&jsonOutput, "json", false,
		"emit machine-readable JSON instead of formatted text (errors also written as JSON to stderr)")
	rootCmd.PersistentFlags().StringVar(&otelProtocol, "otel-protocol", "grpc",
		"OTel receiver protocol: grpc or http (env: TUKITUKI_OTEL_PROTOCOL)")
	rootCmd.PersistentFlags().StringVar(&otelSeverity, "otel-severity", "error",
		"minimum OTel log severity to display (env: TUKITUKI_OTEL_SEVERITY)")
	rootCmd.PersistentFlags().IntVar(&otelPort, "otel-port", 0,
		"OTel receiver port; 0 = random available port (env: TUKITUKI_OTEL_PORT)")

	// Bind persistent flags to viper so env-vars and config file override defaults.
	_ = viper.BindPFlag("run_dir", rootCmd.PersistentFlags().Lookup("run-dir"))
	_ = viper.BindPFlag("state_dir", rootCmd.PersistentFlags().Lookup("state-dir"))
	_ = viper.BindPFlag("otel_protocol", rootCmd.PersistentFlags().Lookup("otel-protocol"))
	_ = viper.BindPFlag("otel_severity", rootCmd.PersistentFlags().Lookup("otel-severity"))
	_ = viper.BindPFlag("otel_port", rootCmd.PersistentFlags().Lookup("otel-port"))

	// Register subcommands.
	rootCmd.AddCommand(
		newVersionCmd(),
		newNewCmd(),
		newListCmd(),
		newStartCmd(),
		newStopCmd(),
		newRestartCmd(),
		newStatusCmd(),
		newLogsCmd(),
		newDebugCmd(),
		newOtelCollectorCmd(),
	)
}

// initConfig sets up viper configuration sources.
func initConfig() {
	viper.SetEnvPrefix("TUKITUKI")
	viper.AutomaticEnv()
	// Env vars use underscores, so TUKITUKI_RUN_DIR maps to run_dir.
	viper.SetEnvKeyReplacer(strings.NewReplacer("-", "_"))

	if cfgFile != "" {
		viper.SetConfigFile(cfgFile)
	} else {
		// Search cwd first, then HOME.
		cwd, _ := os.Getwd()
		viper.AddConfigPath(cwd)
		if home, err := os.UserHomeDir(); err == nil {
			viper.AddConfigPath(home)
		}
		viper.SetConfigName(".tukitukirc")
		viper.SetConfigType("yaml")
	}

	// Silently ignore "config file not found" — it's optional.
	_ = viper.ReadInConfig()
}

// resolveRunDir returns the effective run directory path.
func resolveRunDir() string {
	if d := viper.GetString("run_dir"); d != "" {
		return d
	}
	return ".run"
}

// resolveStateDir returns the effective state directory path.
func resolveStateDir() string {
	if d := viper.GetString("state_dir"); d != "" {
		return d
	}
	return ".tukituki"
}

// resolveProjectRoot returns the absolute path of the current working directory.
func resolveProjectRoot() string {
	root, err := os.Getwd()
	if err != nil {
		return "."
	}
	return root
}

// resolveOtelProtocol returns the effective OTel protocol ("grpc" or "http").
func resolveOtelProtocol() string {
	if p := viper.GetString("otel_protocol"); p != "" {
		return p
	}
	return "grpc"
}

// resolveOtelSeverity returns the effective OTel severity filter name.
func resolveOtelSeverity() string {
	if s := viper.GetString("otel_severity"); s != "" {
		return s
	}
	return "error"
}

// resolveOtelPort returns an explicit OTel port override (--otel-port flag
// or TUKITUKI_OTEL_PORT env), or 0 if none is set. When 0, the Manager
// resolves and persists a stable random port for this state directory.
func resolveOtelPort() int {
	return viper.GetInt("otel_port")
}

// isTTY reports whether stdout is connected to a terminal.
func isTTY() bool {
	fi, err := os.Stdout.Stat()
	if err != nil {
		return false
	}
	return (fi.Mode() & os.ModeCharDevice) != 0
}

// writeJSON marshals v as indented JSON to stdout.
func writeJSON(v any) error {
	data, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal JSON: %w", err)
	}
	fmt.Println(string(data))
	return nil
}

// exitError writes a plain or JSON error to stderr then exits 1.
// extra fields are merged into the JSON object (ignored for plain output).
func exitError(msg string, extra map[string]any) {
	if jsonOutput {
		obj := map[string]any{"error": msg}
		for k, v := range extra {
			obj[k] = v
		}
		data, _ := json.Marshal(obj)
		fmt.Fprintln(os.Stderr, string(data))
	} else {
		fmt.Fprintln(os.Stderr, "Error:", msg)
	}
	os.Exit(1)
}

// loadTargetsOrDie loads run targets, printing a helpful error on failure.
// It also loads a .env file from projectRoot (if present) and expands
// ${VAR} references in each target's env values.
func loadTargetsOrDie(runDirPath, projectRoot string) []config.RunTarget {
	targets, err := config.LoadTargets(runDirPath)
	if err != nil {
		if strings.Contains(err.Error(), "does not exist") {
			exitError(
				fmt.Sprintf("no .run/ directory found at %q — create .run/*.yaml files to define your processes", runDirPath),
				map[string]any{"run_dir": runDirPath},
			)
		} else {
			exitError(fmt.Sprintf("loading targets: %v", err), nil)
		}
	}
	dotenv, err := config.ParseDotEnv(filepath.Join(projectRoot, ".env"))
	if err != nil {
		fmt.Fprintf(os.Stderr, "Warning: could not parse .env: %v\n", err)
	}
	return config.ExpandEnv(targets, dotenv)
}

// findTarget returns the named target or exits with an error listing available names.
// It also recognises the virtual otel-errors target if the manager has it.
func findTarget(targets []config.RunTarget, name string) config.RunTarget {
	for _, t := range targets {
		if t.Name == name {
			return t
		}
	}
	// Accept the virtual otel-errors target even though it is not in the
	// YAML-loaded list — the Manager adds it at runtime.
	if name == process.OtelTargetName {
		return config.RunTarget{
			Name:        process.OtelTargetName,
			Description: "OpenTelemetry error collector",
			Virtual:     true,
		}
	}
	names := make([]string, 0, len(targets))
	for _, t := range targets {
		names = append(names, t.Name)
	}
	exitError(
		fmt.Sprintf("target %q not found", name),
		map[string]any{"available": names},
	)
	return config.RunTarget{} // unreachable
}

// newManagerOrDie creates a process manager, configures OTel settings, and
// exits on error.
func newManagerOrDie(targets []config.RunTarget, stateDirPath string, projectRoot string) *process.Manager {
	mgr, err := process.NewManager(targets, stateDirPath, projectRoot)
	if err != nil {
		exitError(fmt.Sprintf("creating process manager: %v", err), nil)
	}
	mgr.SetOtelConfig(process.OtelConfig{
		Port:     resolveOtelPort(),
		Protocol: resolveOtelProtocol(),
		Severity: resolveOtelSeverity(),
	})
	return mgr
}

// -------------------------------------------------------------------------
// `tukituki version`
// -------------------------------------------------------------------------

func newVersionCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Print version information",
		Long:  "Print the tukituki version, Go runtime version, and OS/architecture.",
		Example: `  tukituki version
  tukituki version --json`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			if jsonOutput {
				return writeJSON(map[string]string{
					"version":    Version,
					"go_version": runtime.Version(),
					"os":         runtime.GOOS,
					"arch":       runtime.GOARCH,
				})
			}
			fmt.Printf("tukituki %s (%s/%s, %s)\n", Version, runtime.GOOS, runtime.GOARCH, runtime.Version())
			return nil
		},
	}
}

// -------------------------------------------------------------------------
// Root command — opens the TUI.
// -------------------------------------------------------------------------

func runRoot(cmd *cobra.Command, args []string) error {
	// Guard: the TUI requires an interactive terminal. Without one (e.g. when
	// called by an AI agent or in a pipeline), exit early with actionable advice
	// rather than blocking indefinitely waiting for input.
	if !isTTY() {
		exitError(
			"no terminal detected — the default command opens an interactive TUI and requires a TTY",
			map[string]any{
				"hint": "use a subcommand for non-interactive use: list, status, start, stop, restart, logs --no-follow",
			},
		)
	}

	runDirPath := resolveRunDir()
	stateDirPath := resolveStateDir()
	projectRoot := resolveProjectRoot()

	targets := loadTargetsOrDie(runDirPath, projectRoot)

	mgr := newManagerOrDie(targets, stateDirPath, projectRoot)

	// Attach to any processes already running from a previous tukituki session.
	if err := mgr.AttachToExisting(); err != nil {
		// Non-fatal: existing state may simply not exist yet.
		fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
	}

	// Start any processes that aren't already running.
	ctx := context.Background()
	if err := mgr.StartAll(ctx); err != nil {
		fmt.Fprintf(os.Stderr, "Warning: some processes failed to start: %v\n", err)
	}

	// Start the OTel collector if any target has otel: true.
	if err := mgr.EnsureOtelCollector(ctx); err != nil {
		fmt.Fprintf(os.Stderr, "Warning: could not start OTel collector: %v\n", err)
	}

	// Use the Manager's target list which may now include the virtual otel-errors entry.
	allTargets := mgr.GetTargets()

	stopAll, err := tui.Start(allTargets, mgr, runDirPath, projectRoot)
	if err != nil {
		return fmt.Errorf("TUI error: %w", err)
	}

	if stopAll {
		if err := mgr.StopAll(); err != nil {
			return fmt.Errorf("stop all: %w", err)
		}
	}

	return nil
}

// -------------------------------------------------------------------------
// `tukituki new <name> '<command> [args...]'`
// -------------------------------------------------------------------------

func newNewCmd() *cobra.Command {
	var envVars []string
	var workdir string

	cmd := &cobra.Command{
		Use:   "new <name> '<command> [args...]'",
		Short: "Create a new run target YAML file",
		Long: `Create a new .run/<name>.yaml file from the given name and command.

The command string is split into the program and its arguments.
Environment variables can be passed with -e KEY=VALUE (repeatable).
Use -w to set a working directory (relative to the project root).`,
		Example: `  tukituki new api 'go run ./cmd/api -port 8080'
  tukituki new worker 'node worker.js' -e PORT=3000 -e DEBUG=true
  tukituki new docs 'hugo server' -w documentation`,
		Args: cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			name := args[0]
			cmdStr := args[1]

			parts := strings.Fields(cmdStr)
			if len(parts) == 0 {
				return fmt.Errorf("command string is empty")
			}

			runDirPath := resolveRunDir()

			// Ensure .run/ directory exists.
			if err := os.MkdirAll(runDirPath, 0o755); err != nil {
				return fmt.Errorf("create run directory: %w", err)
			}

			filePath := filepath.Join(runDirPath, name+".yaml")

			// Don't overwrite an existing file.
			if _, err := os.Stat(filePath); err == nil {
				return fmt.Errorf("file already exists: %s", filePath)
			}

			// Build the YAML content.
			var buf strings.Builder
			buf.WriteString(fmt.Sprintf("name: %s\n", name))
			buf.WriteString(fmt.Sprintf("command: %s\n", parts[0]))
			if workdir != "" {
				buf.WriteString(fmt.Sprintf("workdir: %s\n", workdir))
			}
			if len(parts) > 1 {
				buf.WriteString("args:\n")
				for _, a := range parts[1:] {
					buf.WriteString(fmt.Sprintf("  - %s\n", a))
				}
			}
			if len(envVars) > 0 {
				buf.WriteString("env:\n")
				for _, e := range envVars {
					k, v, ok := strings.Cut(e, "=")
					if !ok {
						return fmt.Errorf("invalid env var format %q (expected KEY=VALUE)", e)
					}
					buf.WriteString(fmt.Sprintf("  %s: %q\n", k, v))
				}
			}

			if err := os.WriteFile(filePath, []byte(buf.String()), 0o644); err != nil {
				return fmt.Errorf("write file: %w", err)
			}

			if jsonOutput {
				return writeJSON(map[string]string{
					"file": filePath,
					"name": name,
				})
			}
			fmt.Printf("Created %s\n", filePath)
			return nil
		},
	}

	cmd.Flags().StringArrayVarP(&envVars, "env", "e", nil,
		"environment variable in KEY=VALUE format (repeatable)")
	cmd.Flags().StringVarP(&workdir, "workdir", "w", "",
		"working directory relative to project root")

	return cmd
}

// -------------------------------------------------------------------------
// `tukituki list`
// -------------------------------------------------------------------------

type listEntry struct {
	Name        string   `json:"name"`
	Command     string   `json:"command"`
	Args        []string `json:"args,omitempty"`
	Description string   `json:"description,omitempty"`
	Workdir     string   `json:"workdir,omitempty"`
}

func newListCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "list",
		Short: "List all configured run targets",
		Long: `List all run targets defined in .run/*.yaml.

Outputs name, command, and description for each target.
Use --json for machine-readable output.`,
		Example: `  tukituki list
  tukituki list --json`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			targets := loadTargetsOrDie(resolveRunDir(), resolveProjectRoot())

			if jsonOutput {
				entries := make([]listEntry, len(targets))
				for i, t := range targets {
					entries[i] = listEntry{
						Name:        t.Name,
						Command:     t.Command,
						Args:        t.Args,
						Description: t.Description,
						Workdir:     t.Workdir,
					}
				}
				return writeJSON(entries)
			}

			w := tabwriter.NewWriter(os.Stdout, 0, 0, 3, ' ', 0)
			fmt.Fprintln(w, "NAME\tCOMMAND\tDESCRIPTION")
			fmt.Fprintln(w, "----\t-------\t-----------")
			for _, t := range targets {
				desc := t.Description
				if desc == "" {
					desc = "-"
				}
				fmt.Fprintf(w, "%s\t%s\t%s\n", t.Name, t.Command, desc)
			}
			return w.Flush()
		},
	}
}

// -------------------------------------------------------------------------
// `tukituki status [target-name]`
// -------------------------------------------------------------------------

type statusEntry struct {
	Name        string `json:"name"`
	Status      string `json:"status"`
	Description string `json:"description,omitempty"`
	PID         int    `json:"pid,omitempty"`
	Address     string `json:"address,omitempty"`
}

func newStatusCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "status [target-name]",
		Short: "Print the status of all targets (or a single target)",
		Long: `Print the runtime status of managed processes.

With no argument all targets are shown. Pass a target name to query one.
Status values: running, stopped, failed, unknown.
Use --json for machine-readable output.`,
		Example: `  tukituki status
  tukituki status api
  tukituki status --json
  tukituki status api --json`,
		Args: cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()
			projectRoot := resolveProjectRoot()

			targets := loadTargetsOrDie(runDirPath, projectRoot)
			mgr := newManagerOrDie(targets, stateDirPath, projectRoot)

			if err := mgr.AttachToExisting(); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
			}

			// Use the manager's post-attach target list so virtual targets
			// (e.g. otel-errors) registered by AttachToExisting are visible.
			targets = mgr.GetTargets()

			// Filter to a single target if provided.
			if len(args) == 1 {
				t := findTarget(targets, args[0])
				targets = []config.RunTarget{t}
			}

			statuses := mgr.GetAllStatuses()
			states := mgr.GetAllProcessStates()
			otelPort := mgr.OtelReceiverPort()

			if jsonOutput {
				entries := make([]statusEntry, 0, len(targets))
				for _, t := range targets {
					status, ok := statuses[t.Name]
					if !ok {
						status = "unknown"
					}
					entry := statusEntry{
						Name:        t.Name,
						Status:      string(status),
						Description: t.Description,
					}
					if ps, ok := states[t.Name]; ok && ps != nil {
						entry.PID = ps.PID
					}
					if t.Name == process.OtelTargetName && otelPort != 0 && status == state.StatusRunning {
						entry.Address = fmt.Sprintf("127.0.0.1:%d", otelPort)
					}
					entries = append(entries, entry)
				}
				if len(args) == 1 && len(entries) == 1 {
					return writeJSON(entries[0])
				}
				return writeJSON(entries)
			}

			w := tabwriter.NewWriter(os.Stdout, 0, 0, 3, ' ', 0)
			fmt.Fprintln(w, "NAME\tSTATUS\tDESCRIPTION")
			fmt.Fprintln(w, "----\t------\t-----------")
			for _, t := range targets {
				status, ok := statuses[t.Name]
				if !ok {
					status = "unknown"
				}
				desc := t.Description
				if desc == "" {
					desc = "-"
				}
				if t.Name == process.OtelTargetName && otelPort != 0 && status == state.StatusRunning {
					desc = fmt.Sprintf("%s (listening on :%d)", desc, otelPort)
				}
				fmt.Fprintf(w, "%s\t%s\t%s\n", t.Name, status, desc)
			}
			return w.Flush()
		},
	}
}

// -------------------------------------------------------------------------
// `tukituki start [target-name]`
// -------------------------------------------------------------------------

type actionResult struct {
	Name   string `json:"name"`
	Status string `json:"status"`
}

func newStartCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "start [target-name]",
		Short: "Start one or all targets (headless, no TUI)",
		Long: `Start one or all targets as background processes (no TUI).

If target-name is omitted, all configured targets are started.
Processes that are already running are left untouched.
Use --json for machine-readable output.`,
		Example: `  tukituki start
  tukituki start api
  tukituki start api --json`,
		Args: cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()
			projectRoot := resolveProjectRoot()

			targets := loadTargetsOrDie(runDirPath, projectRoot)
			mgr := newManagerOrDie(targets, stateDirPath, projectRoot)

			ctx := context.Background()

			if len(args) == 1 {
				name := args[0]
				_ = findTarget(targets, name)
				if err := mgr.Start(ctx, name); err != nil {
					return fmt.Errorf("start %q: %w", name, err)
				}
				return printActionResult(mgr, name)
			}

			if err := mgr.StartAll(ctx); err != nil {
				return fmt.Errorf("start all: %w", err)
			}

			// Start the OTel collector if any target has otel: true.
			if err := mgr.EnsureOtelCollector(ctx); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not start OTel collector: %v\n", err)
			}

			allTargets := mgr.GetTargets()
			if jsonOutput {
				results := make([]actionResult, 0, len(allTargets))
				statuses := mgr.GetAllStatuses()
				for _, t := range allTargets {
					st := statuses[t.Name]
					results = append(results, actionResult{Name: t.Name, Status: string(st)})
				}
				return writeJSON(results)
			}
			for _, t := range allTargets {
				printStartedText(mgr, t.Name)
			}
			return nil
		},
	}
}

func printActionResult(mgr *process.Manager, name string) error {
	statuses := mgr.GetAllStatuses()
	st := statuses[name]
	if jsonOutput {
		return writeJSON(actionResult{Name: name, Status: string(st)})
	}
	fmt.Printf("Started: %s (status: %s)\n", name, st)
	return nil
}

func printStartedText(mgr *process.Manager, name string) {
	statuses := mgr.GetAllStatuses()
	status, ok := statuses[name]
	if ok {
		fmt.Printf("Started: %s (status: %s)\n", name, status)
	} else {
		fmt.Printf("Started: %s\n", name)
	}
}

// -------------------------------------------------------------------------
// `tukituki stop [target-name]`
// -------------------------------------------------------------------------

func newStopCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "stop [target-name]",
		Short: "Stop one or all targets",
		Long: `Stop one or all running targets.

Sends SIGTERM, waits up to 5 seconds, then SIGKILLs if still running.
If target-name is omitted, all targets are stopped.
Use --json for machine-readable output.`,
		Example: `  tukituki stop
  tukituki stop api
  tukituki stop api --json`,
		Args: cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()
			projectRoot := resolveProjectRoot()

			targets := loadTargetsOrDie(runDirPath, projectRoot)
			mgr := newManagerOrDie(targets, stateDirPath, projectRoot)

			if err := mgr.AttachToExisting(); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
			}

			if len(args) == 1 {
				name := args[0]
				_ = findTarget(targets, name)
				if err := mgr.Stop(name); err != nil {
					return fmt.Errorf("stop %q: %w", name, err)
				}
				if jsonOutput {
					return writeJSON(actionResult{Name: name, Status: "stopped"})
				}
				fmt.Printf("Stopped: %s\n", name)
				return nil
			}

			if err := mgr.StopAll(); err != nil {
				return fmt.Errorf("stop all: %w", err)
			}

			if jsonOutput {
				results := make([]actionResult, len(targets))
				for i, t := range targets {
					results[i] = actionResult{Name: t.Name, Status: "stopped"}
				}
				return writeJSON(results)
			}
			for _, t := range targets {
				fmt.Printf("Stopped: %s\n", t.Name)
			}
			return nil
		},
	}
}

// -------------------------------------------------------------------------
// `tukituki restart <target-name>`
// -------------------------------------------------------------------------

func newRestartCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "restart <target-name> [target-name ...]",
		Short: "Restart one or more targets",
		Long: `Stop and then start each named target, in order.

If a process is not currently running, it is simply started.
All names are validated up front; if any is unknown the command exits
before restarting anything. Use --json for machine-readable output.`,
		Example: `  tukituki restart api
  tukituki restart api worker
  tukituki restart api --json`,
		Args: cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()
			projectRoot := resolveProjectRoot()

			targets := loadTargetsOrDie(runDirPath, projectRoot)
			mgr := newManagerOrDie(targets, stateDirPath, projectRoot)

			if err := mgr.AttachToExisting(); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
			}

			// Validate every name before restarting anything so a typo in
			// the last arg doesn't leave earlier targets bounced.
			for _, name := range args {
				_ = findTarget(targets, name)
			}

			ctx := context.Background()
			for _, name := range args {
				if err := mgr.Restart(ctx, name); err != nil {
					return fmt.Errorf("restart %q: %w", name, err)
				}
			}

			statuses := mgr.GetAllStatuses()
			if jsonOutput {
				results := make([]actionResult, len(args))
				for i, name := range args {
					results[i] = actionResult{Name: name, Status: string(statuses[name])}
				}
				if len(results) == 1 {
					return writeJSON(results[0])
				}
				return writeJSON(results)
			}
			for _, name := range args {
				fmt.Printf("Restarted: %s (status: %s)\n", name, statuses[name])
			}
			return nil
		},
	}
}

// -------------------------------------------------------------------------
// `tukituki debug [target-name]`
// -------------------------------------------------------------------------

func newDebugCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:    "debug [target-name]",
		Short:  "Show the resolved configuration and shell command for targets",
		Hidden: true,
		Args:   cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			projectRoot := resolveProjectRoot()

			targets := loadTargetsOrDie(runDirPath, projectRoot)

			if len(args) == 1 {
				t := findTarget(targets, args[0])
				targets = []config.RunTarget{t}
			}

			shell := os.Getenv("SHELL")
			if shell == "" {
				shell = "/bin/sh"
			}

			if jsonOutput {
				type debugEntry struct {
					Name     string            `json:"name"`
					Command  string            `json:"command"`
					Args     []string          `json:"args"`
					Workdir  string            `json:"workdir,omitempty"`
					Env      map[string]string `json:"env,omitempty"`
					Cleanup  []string          `json:"cleanup,omitempty"`
					ShellCmd string            `json:"shell_cmd"`
					ShellArg string            `json:"shell_arg"`
				}
				entries := make([]debugEntry, 0, len(targets))
				for _, t := range targets {
					shellLine := process.BuildShellCmd(t.Command, t.Args)
					entries = append(entries, debugEntry{
						Name:     t.Name,
						Command:  t.Command,
						Args:     t.Args,
						Workdir:  t.Workdir,
						Env:      t.Env,
						Cleanup:  t.Cleanup,
						ShellCmd: shell + " -l -c",
						ShellArg: shellLine,
					})
				}
				if len(args) == 1 && len(entries) == 1 {
					return writeJSON(entries[0])
				}
				return writeJSON(entries)
			}

			for i, t := range targets {
				if i > 0 {
					fmt.Println()
				}
				shellLine := process.BuildShellCmd(t.Command, t.Args)

				fmt.Printf("Target:   %s\n", t.Name)
				fmt.Printf("Command:  %s\n", t.Command)
				if len(t.Args) > 0 {
					for j, a := range t.Args {
						if a == "" {
							fmt.Printf("  arg[%d]:  \"\" (empty string)\n", j)
						} else {
							fmt.Printf("  arg[%d]:  %s\n", j, a)
						}
					}
				}
				if t.Workdir != "" {
					wd := t.Workdir
					if !filepath.IsAbs(wd) {
						wd = filepath.Join(projectRoot, wd)
					}
					fmt.Printf("Workdir:  %s\n", wd)
				}
				if len(t.Env) > 0 {
					fmt.Println("Env:")
					for k, v := range t.Env {
						fmt.Printf("  %s=%s\n", k, v)
					}
				}
				if len(t.Cleanup) > 0 {
					fmt.Println("Cleanup:")
					for _, c := range t.Cleanup {
						fmt.Printf("  %s\n", c)
					}
				}
				fmt.Printf("Shell:    %s -l -c %s\n", shell, shellLine)
			}
			return nil
		},
	}
	return cmd
}

// -------------------------------------------------------------------------
// `tukituki logs <target-name>`
// -------------------------------------------------------------------------

func newLogsCmd() *cobra.Command {
	var noFollow bool
	var tail int

	cmd := &cobra.Command{
		Use:   "logs <target-name>",
		Short: "Tail logs for a target",
		Long: `Print recent log lines for a target and optionally follow new output.

By default prints the last 100 lines and then streams new lines until Ctrl+C.
Use --no-follow to print buffered lines and exit immediately (safe for scripts
and AI agents).  Use --tail to control how many buffered lines are shown.`,
		Example: `  tukituki logs api
  tukituki logs api --no-follow
  tukituki logs api --tail 50 --no-follow`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()
			projectRoot := resolveProjectRoot()

			targets := loadTargetsOrDie(runDirPath, projectRoot)
			mgr := newManagerOrDie(targets, stateDirPath, projectRoot)

			if err := mgr.AttachToExisting(); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
			}

			name := args[0]
			_ = findTarget(targets, name)

			if noFollow {
				// Read the log file directly from disk — the async
				// tailer may not have populated the ring buffer yet.
				states := mgr.GetAllProcessStates()
				ps, ok := states[name]
				if !ok || ps.LogFile == "" {
					return nil
				}
				data, err := os.ReadFile(ps.LogFile)
				if err != nil {
					if os.IsNotExist(err) {
						return nil
					}
					return err
				}
				content := strings.ReplaceAll(string(data), "\x00", "")
				lines := strings.Split(content, "\n")
				if len(lines) > 0 && lines[len(lines)-1] == "" {
					lines = lines[:len(lines)-1]
				}
				start := 0
				if tail > 0 && len(lines) > tail {
					start = len(lines) - tail
				}
				w := bufio.NewWriter(os.Stdout)
				for _, line := range lines[start:] {
					fmt.Fprintln(w, line)
				}
				return w.Flush()
			}

			// Print buffered lines up to --tail limit.
			buffered := mgr.GetLogLines(name)
			start := 0
			if tail > 0 && len(buffered) > tail {
				start = len(buffered) - tail
			}
			w := bufio.NewWriter(os.Stdout)
			for _, line := range buffered[start:] {
				fmt.Fprintln(w, line)
			}
			_ = w.Flush()

			// Follow new lines until Ctrl+C.
			ch := mgr.WatchLogLines(name)

			sigCh := make(chan os.Signal, 1)
			signal.Notify(sigCh, os.Interrupt, syscall.SIGTERM)
			defer signal.Stop(sigCh)

			for {
				select {
				case line, ok := <-ch:
					if !ok {
						// Channel closed — process stopped.
						return nil
					}
					fmt.Println(line)
				case <-sigCh:
					return nil
				}
			}
		},
	}

	cmd.Flags().BoolVar(&noFollow, "no-follow", false,
		"print buffered log lines and exit without streaming (safe for scripts and agents)")
	cmd.Flags().IntVar(&tail, "tail", 100,
		"number of buffered log lines to print (0 = all)")

	return cmd
}

// -------------------------------------------------------------------------
// `tukituki otel-collector` (hidden — spawned automatically by the manager)
// -------------------------------------------------------------------------

func newOtelCollectorCmd() *cobra.Command {
	var protocol string
	var severity string
	var port int
	var notifySocket string

	cmd := &cobra.Command{
		Use:    "otel-collector",
		Short:  "Run the embedded OTLP log receiver (internal use)",
		Hidden: true,
		Args:   cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			minSev, err := otelPkg.ParseSeverity(severity)
			if err != nil {
				return err
			}

			ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
			defer cancel()

			c := &otelPkg.Collector{
				Port:         port,
				Protocol:     protocol,
				MinSeverity:  minSev,
				NotifySocket: notifySocket,
			}
			return c.Run(ctx)
		},
	}

	cmd.Flags().StringVar(&protocol, "protocol", "grpc", "receiver protocol: grpc or http")
	cmd.Flags().StringVar(&severity, "severity", "error", "minimum log severity to emit")
	cmd.Flags().IntVar(&port, "port", 4317, "port to listen on")
	cmd.Flags().StringVar(&notifySocket, "notify-socket", "",
		"unix socket path on which to publish error notifications to the parent TUI (empty = disabled)")

	return cmd
}
