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

package tui

import (
	"context"
	"net"
	"time"

	notifypb "github.com/dvaldivia/tukituki/internal/otel/notify"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

// runOtelNotifySubscriber dials the otel-collector's notification socket and
// pumps every received ErrorEvent onto out. It runs until ctx is cancelled,
// retrying the dial and the Subscribe stream forever so a collector restart
// (or a TUI started before the collector is ready) is recovered transparently.
//
// Emits at most one event per OTLP record that passed the collector's
// severity filter; nothing is buffered locally.
func runOtelNotifySubscriber(ctx context.Context, socket string, out chan<- *notifypb.ErrorEvent) {
	defer close(out)

	for {
		if ctx.Err() != nil {
			return
		}
		conn, err := grpc.NewClient(
			"unix:"+socket,
			grpc.WithTransportCredentials(insecure.NewCredentials()),
			grpc.WithContextDialer(func(ctx context.Context, addr string) (net.Conn, error) {
				var d net.Dialer
				return d.DialContext(ctx, "unix", socket)
			}),
		)
		if err != nil {
			if !sleep(ctx, time.Second) {
				return
			}
			continue
		}

		client := notifypb.NewNotifierClient(conn)
		stream, err := client.Subscribe(ctx, &notifypb.SubscribeRequest{})
		if err != nil {
			conn.Close()
			if !sleep(ctx, time.Second) {
				return
			}
			continue
		}

		for {
			ev, err := stream.Recv()
			if err != nil {
				break
			}
			select {
			case out <- ev:
			case <-ctx.Done():
				conn.Close()
				return
			}
		}
		conn.Close()
		if !sleep(ctx, 500*time.Millisecond) {
			return
		}
	}
}

// sleep blocks for d, returning false if ctx is cancelled first.
func sleep(ctx context.Context, d time.Duration) bool {
	t := time.NewTimer(d)
	defer t.Stop()
	select {
	case <-t.C:
		return true
	case <-ctx.Done():
		return false
	}
}
