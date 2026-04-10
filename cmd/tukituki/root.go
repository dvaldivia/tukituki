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
	"fmt"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"text/tabwriter"

	"github.com/dvaldivia/tukituki/internal/config"
	"github.com/dvaldivia/tukituki/internal/process"
	"github.com/dvaldivia/tukituki/internal/tui"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

var (
	cfgFile  string
	runDir   string
	stateDir string
)

// rootCmd is the base command; when called with no subcommand it starts the TUI.
var rootCmd = &cobra.Command{
	Use:   "tukituki",
	Short: "tukituki — manage multiple dev processes from a TUI",
	Long: `tukituki reads process definitions from .run/*.yaml and lets you
start, stop, restart, and tail their logs from an interactive TUI.

Run with no arguments to open the TUI. Use subcommands for headless control.`,
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
		"directory containing YAML run definitions (default: .run)")
	rootCmd.PersistentFlags().StringVar(&stateDir, "state-dir", "",
		"directory for state file and logs (default: .tukituki)")

	// Bind persistent flags to viper so env-vars and config file override defaults.
	_ = viper.BindPFlag("run_dir", rootCmd.PersistentFlags().Lookup("run-dir"))
	_ = viper.BindPFlag("state_dir", rootCmd.PersistentFlags().Lookup("state-dir"))

	// Register subcommands.
	rootCmd.AddCommand(
		newListCmd(),
		newStartCmd(),
		newStopCmd(),
		newRestartCmd(),
		newStatusCmd(),
		newLogsCmd(),
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

// resolveProjectRoot returns the absolute path of the current working directory,
// which is used to resolve relative workdir values in .run/*.yaml files.
func resolveProjectRoot() string {
	root, err := os.Getwd()
	if err != nil {
		return "."
	}
	return root
}

// loadTargetsOrDie loads run targets, printing a helpful error on failure.
func loadTargetsOrDie(runDirPath string) []config.RunTarget {
	targets, err := config.LoadTargets(runDirPath)
	if err != nil {
		if strings.Contains(err.Error(), "does not exist") {
			fmt.Fprintf(os.Stderr,
				"Error: No .run/ directory found at %q.\n"+
					"Create .run/*.yaml files to define your processes.\n", runDirPath)
		} else {
			fmt.Fprintf(os.Stderr, "Error loading targets: %v\n", err)
		}
		os.Exit(1)
	}
	return targets
}

// findTarget returns the named target or exits with an error listing available names.
func findTarget(targets []config.RunTarget, name string) config.RunTarget {
	for _, t := range targets {
		if t.Name == name {
			return t
		}
	}
	names := make([]string, 0, len(targets))
	for _, t := range targets {
		names = append(names, t.Name)
	}
	fmt.Fprintf(os.Stderr,
		"Error: target %q not found.\nAvailable targets: %s\n",
		name, strings.Join(names, ", "))
	os.Exit(1)
	return config.RunTarget{} // unreachable
}

// newManagerOrDie creates a process manager and exits on error.
// projectRoot is the directory relative to which workdir values are resolved;
// pass os.Getwd() at the call site.
func newManagerOrDie(targets []config.RunTarget, stateDirPath string, projectRoot string) *process.Manager {
	mgr, err := process.NewManager(targets, stateDirPath, projectRoot)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error creating process manager: %v\n", err)
		os.Exit(1)
	}
	return mgr
}

// -------------------------------------------------------------------------
// Root command — opens the TUI.
// -------------------------------------------------------------------------

func runRoot(cmd *cobra.Command, args []string) error {
	runDirPath := resolveRunDir()
	stateDirPath := resolveStateDir()

	targets := loadTargetsOrDie(runDirPath)

	mgr := newManagerOrDie(targets, stateDirPath, resolveProjectRoot())

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

	stopAll, err := tui.Start(targets, mgr)
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
// `tukituki start [target-name]`
// -------------------------------------------------------------------------

func newStartCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "start [target-name]",
		Short: "Start one or all targets (headless, no TUI)",
		Args:  cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()

			targets := loadTargetsOrDie(runDirPath)
			mgr := newManagerOrDie(targets, stateDirPath, resolveProjectRoot())

			ctx := context.Background()

			if len(args) == 1 {
				name := args[0]
				_ = findTarget(targets, name) // validate name exists
				if err := mgr.Start(ctx, name); err != nil {
					return fmt.Errorf("start %q: %w", name, err)
				}
				printStarted(mgr, name)
			} else {
				if err := mgr.StartAll(ctx); err != nil {
					return fmt.Errorf("start all: %w", err)
				}
				for _, t := range targets {
					printStarted(mgr, t.Name)
				}
			}
			return nil
		},
	}
}

// printStarted prints "Started: {name} (PID {pid})" by consulting the manager statuses.
// Since GetAllStatuses returns state.Status (not the PID directly), we print what we can.
func printStarted(mgr *process.Manager, name string) {
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
		Args:  cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()

			targets := loadTargetsOrDie(runDirPath)
			mgr := newManagerOrDie(targets, stateDirPath, resolveProjectRoot())

			if err := mgr.AttachToExisting(); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
			}

			if len(args) == 1 {
				name := args[0]
				_ = findTarget(targets, name)
				if err := mgr.Stop(name); err != nil {
					return fmt.Errorf("stop %q: %w", name, err)
				}
				fmt.Printf("Stopped: %s\n", name)
			} else {
				if err := mgr.StopAll(); err != nil {
					return fmt.Errorf("stop all: %w", err)
				}
				for _, t := range targets {
					fmt.Printf("Stopped: %s\n", t.Name)
				}
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
		Use:   "restart <target-name>",
		Short: "Restart a target",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()

			targets := loadTargetsOrDie(runDirPath)
			mgr := newManagerOrDie(targets, stateDirPath, resolveProjectRoot())

			if err := mgr.AttachToExisting(); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
			}

			name := args[0]
			_ = findTarget(targets, name)

			ctx := context.Background()
			if err := mgr.Restart(ctx, name); err != nil {
				return fmt.Errorf("restart %q: %w", name, err)
			}
			fmt.Printf("Restarted: %s\n", name)
			return nil
		},
	}
}

// -------------------------------------------------------------------------
// `tukituki status`
// -------------------------------------------------------------------------

func newStatusCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "status",
		Short: "Print the status of all targets",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()

			targets := loadTargetsOrDie(runDirPath)
			mgr := newManagerOrDie(targets, stateDirPath, resolveProjectRoot())

			if err := mgr.AttachToExisting(); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
			}

			statuses := mgr.GetAllStatuses()

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
				fmt.Fprintf(w, "%s\t%s\t%s\n", t.Name, status, desc)
			}
			return w.Flush()
		},
	}
}

// -------------------------------------------------------------------------
// `tukituki list`
// -------------------------------------------------------------------------

func newListCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "list",
		Short: "List all configured run targets",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			targets := loadTargetsOrDie(resolveRunDir())

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
// `tukituki logs <target-name>`
// -------------------------------------------------------------------------

func newLogsCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "logs <target-name>",
		Short: "Tail logs for a target (last 100 lines, then follow)",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			runDirPath := resolveRunDir()
			stateDirPath := resolveStateDir()

			targets := loadTargetsOrDie(runDirPath)
			mgr := newManagerOrDie(targets, stateDirPath, resolveProjectRoot())

			if err := mgr.AttachToExisting(); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not attach to existing processes: %v\n", err)
			}

			name := args[0]
			_ = findTarget(targets, name)

			// Print buffered lines (last 100).
			buffered := mgr.GetLogLines(name)
			start := 0
			if len(buffered) > 100 {
				start = len(buffered) - 100
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
}
