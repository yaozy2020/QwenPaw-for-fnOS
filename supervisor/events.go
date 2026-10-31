package main

// 变更广播(dsh update.go 模式):订阅者各持 buffered(1) 队列,
// notify 非阻塞投递自动合并突发;连接建立即发首帧全量快照。

import "sync"

type Broadcaster struct {
	mu   sync.Mutex
	subs map[chan struct{}]struct{}
}

func NewBroadcaster() *Broadcaster {
	return &Broadcaster{subs: make(map[chan struct{}]struct{})}
}

func (b *Broadcaster) Subscribe() (ch chan struct{}, unsub func()) {
	ch = make(chan struct{}, 1)
	b.mu.Lock()
	b.subs[ch] = struct{}{}
	b.mu.Unlock()
	return ch, func() {
		b.mu.Lock()
		delete(b.subs, ch)
		b.mu.Unlock()
	}
}

// Notify 非阻塞投递:慢消费者自动合并突发(dsh server.go:398-409 同款)。
func (b *Broadcaster) Notify() {
	b.mu.Lock()
	defer b.mu.Unlock()
	for ch := range b.subs {
		select {
		case ch <- struct{}{}:
		default:
		}
	}
}
