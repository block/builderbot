package watcher

import (
	"testing"

	"github.com/loganj/penpal/internal/activity"
	"github.com/loganj/penpal/internal/cache"
)

// E-PENPAL-WATCHER: verifies Subscribe returns a channel that receives broadcast events.
func TestSubscribeBroadcast(t *testing.T) {
	c := cache.New()
	w, err := New(c, activity.New())
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer w.Stop()

	ch := w.Subscribe()

	evt := Event{Type: EventFilesChanged, Project: "p"}
	w.Broadcast(evt)

	select {
	case got := <-ch:
		if got.Type != evt.Type {
			t.Errorf("expected type %q, got %q", evt.Type, got.Type)
		}
		if got.Project != evt.Project {
			t.Errorf("expected project %q, got %q", evt.Project, got.Project)
		}
	default:
		t.Fatal("expected event on channel, got nothing")
	}

	w.Unsubscribe(ch)

	// After unsubscribe the channel should be closed
	_, open := <-ch
	if open {
		t.Error("expected channel to be closed after Unsubscribe")
	}
}

// E-PENPAL-WATCHER: verifies broadcast delivers to all subscribers.
func TestBroadcastMultipleSubscribers(t *testing.T) {
	c := cache.New()
	w, err := New(c, activity.New())
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer w.Stop()

	ch1 := w.Subscribe()
	ch2 := w.Subscribe()
	ch3 := w.Subscribe()

	evt := Event{Type: EventCommentsChanged, Project: "multi"}
	w.Broadcast(evt)

	for i, ch := range []chan Event{ch1, ch2, ch3} {
		select {
		case got := <-ch:
			if got.Type != evt.Type {
				t.Errorf("subscriber %d: expected type %q, got %q", i, evt.Type, got.Type)
			}
			if got.Project != evt.Project {
				t.Errorf("subscriber %d: expected project %q, got %q", i, evt.Project, got.Project)
			}
		default:
			t.Errorf("subscriber %d: expected event, got nothing", i)
		}
	}

	w.Unsubscribe(ch1)
	w.Unsubscribe(ch2)
	w.Unsubscribe(ch3)
}

// E-PENPAL-WATCHER: verifies broadcast after unsubscribe does not panic.
func TestBroadcastAfterUnsubscribe(t *testing.T) {
	c := cache.New()
	w, err := New(c, activity.New())
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer w.Stop()

	ch := w.Subscribe()
	w.Unsubscribe(ch)

	// Broadcasting with no subscribers should not panic
	w.Broadcast(Event{Type: EventProjectsChanged, Project: "gone"})
}

// E-PENPAL-WATCHER: verifies broadcast is non-blocking when channel buffer is full.
func TestBroadcastNonBlocking(t *testing.T) {
	c := cache.New()
	w, err := New(c, activity.New())
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer w.Stop()

	ch := w.Subscribe() // channel capacity is 10

	// Broadcast 15 events rapidly (more than channel capacity of 10)
	for i := 0; i < 15; i++ {
		w.Broadcast(Event{Type: EventFilesChanged, Project: "burst"})
	}

	// Read all available events from the channel
	count := 0
	for {
		select {
		case <-ch:
			count++
		default:
			goto done
		}
	}
done:

	if count > 10 {
		t.Errorf("expected at most 10 events (channel capacity), got %d", count)
	}
	if count == 0 {
		t.Error("expected at least some events to be delivered")
	}

	w.Unsubscribe(ch)
}
