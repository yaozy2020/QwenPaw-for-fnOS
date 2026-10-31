package main

// SSE 基元 —— 模式照搬 dsh backend/sse.go(MIT):
//   - X-Accel-Buffering: no 对抗网关缓冲
//   - 15s 心跳发 ": ping" 注释行(fnOS 网关 300s read_timeout 会掐空闲连接)
//   - 首帧必须全量快照,前端重连靠它追平,无需补发逻辑

import (
	"encoding/json"
	"net/http"
	"time"
)

func setSSEHeaders(w http.ResponseWriter) {
	h := w.Header()
	h.Set("Content-Type", "text/event-stream")
	h.Set("Cache-Control", "no-cache")
	h.Set("Connection", "keep-alive")
	h.Set("X-Accel-Buffering", "no")
	h.Set("Accept-Encoding", "identity")
}

func sseSend(w http.ResponseWriter, event string, data any) bool {
	b, err := json.Marshal(data)
	if err != nil {
		return false
	}
	_, err = w.Write([]byte("event: " + event + "\ndata: " + string(b) + "\n\n"))
	if f, ok := w.(http.Flusher); ok {
		f.Flush()
	}
	return err == nil
}

// ssePing 写注释行:产生真实字节保活,但不触发前端业务事件。
func ssePing(w http.ResponseWriter) {
	_, _ = w.Write([]byte(": ping " + time.Now().UTC().Format(time.RFC3339) + "\n\n"))
	if f, ok := w.(http.Flusher); ok {
		f.Flush()
	}
}
