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

package otel

import (
	"context"
	"sync"

	notifypb "github.com/dvaldivia/tukituki/internal/otel/notify"
)

// notifyHub fan-outs ErrorEvents to every connected Subscribe stream.
// It is safe for concurrent use. Publish is non-blocking: a slow
// subscriber drops events instead of stalling the OTLP receive path.
type notifyHub struct {
	notifypb.UnimplementedNotifierServer

	mu   sync.RWMutex
	subs map[chan *notifypb.ErrorEvent]struct{}
}

func newNotifyHub() *notifyHub {
	return &notifyHub{subs: make(map[chan *notifypb.ErrorEvent]struct{})}
}

// Publish delivers ev to all current subscribers, dropping for any subscriber
// whose buffer is full. Returns immediately.
func (h *notifyHub) Publish(ev *notifypb.ErrorEvent) {
	h.mu.RLock()
	defer h.mu.RUnlock()
	for ch := range h.subs {
		select {
		case ch <- ev:
		default:
			// Subscriber is behind; drop rather than block the OTLP path.
		}
	}
}

func (h *notifyHub) addSubscriber() chan *notifypb.ErrorEvent {
	ch := make(chan *notifypb.ErrorEvent, 256)
	h.mu.Lock()
	h.subs[ch] = struct{}{}
	h.mu.Unlock()
	return ch
}

func (h *notifyHub) removeSubscriber(ch chan *notifypb.ErrorEvent) {
	h.mu.Lock()
	delete(h.subs, ch)
	h.mu.Unlock()
	close(ch)
}

// Subscribe streams ErrorEvents to the caller until it disconnects or the
// collector shuts down.
func (h *notifyHub) Subscribe(_ *notifypb.SubscribeRequest, stream notifypb.Notifier_SubscribeServer) error {
	ch := h.addSubscriber()
	defer h.removeSubscriber(ch)

	ctx := stream.Context()
	for {
		select {
		case <-ctx.Done():
			return nil
		case ev, ok := <-ch:
			if !ok {
				return nil
			}
			if err := stream.Send(ev); err != nil {
				return err
			}
		}
	}
}

// shutdown closes every active subscriber channel; outstanding Subscribe
// goroutines exit on the next loop iteration. Safe to call once at server
// teardown.
func (h *notifyHub) shutdown(_ context.Context) {
	h.mu.Lock()
	defer h.mu.Unlock()
	for ch := range h.subs {
		delete(h.subs, ch)
		close(ch)
	}
}
