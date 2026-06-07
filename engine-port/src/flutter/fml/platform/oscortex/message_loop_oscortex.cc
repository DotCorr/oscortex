// Copyright 2013 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// OSCortex native port — see message_loop_oscortex.h. Currently identical to the
// linux epoll+timerfd loop (OSCortex is linux-linked for now); the wait/timer
// calls are the divergence point to OSCortex native primitives. Reuses the
// shared timerfd helpers from platform/linux/timerfd.h.

#include "flutter/fml/platform/oscortex/message_loop_oscortex.h"

#include <sys/epoll.h>
#include <unistd.h>

#include "flutter/fml/eintr_wrapper.h"
#include "flutter/fml/platform/linux/timerfd.h"

namespace fml {

static constexpr int kClockType = CLOCK_MONOTONIC;

MessageLoopOscortex::MessageLoopOscortex()
    : epoll_fd_(FML_HANDLE_EINTR(::epoll_create(1 /* unused */))),
      timer_fd_(::timerfd_create(kClockType, TFD_NONBLOCK | TFD_CLOEXEC)) {
  FML_CHECK(epoll_fd_.is_valid());
  FML_CHECK(timer_fd_.is_valid());
  bool added_source = AddOrRemoveTimerSource(true);
  FML_CHECK(added_source);
}

MessageLoopOscortex::~MessageLoopOscortex() {
  bool removed_source = AddOrRemoveTimerSource(false);
  FML_CHECK(removed_source);
}

bool MessageLoopOscortex::AddOrRemoveTimerSource(bool add) {
  struct epoll_event event = {};

  event.events = EPOLLIN;
  // The data is just for informational purposes so we know when we were woken
  // by the FD.
  event.data.fd = timer_fd_.get();

  int ctl_result =
      ::epoll_ctl(epoll_fd_.get(), add ? EPOLL_CTL_ADD : EPOLL_CTL_DEL,
                  timer_fd_.get(), &event);
  return ctl_result == 0;
}

// |fml::MessageLoopImpl|
void MessageLoopOscortex::Run() {
  running_ = true;

  while (running_) {
    struct epoll_event event = {};

    int epoll_result = FML_HANDLE_EINTR(
        ::epoll_wait(epoll_fd_.get(), &event, 1, -1 /* timeout */));

    // Errors are fatal.
    if (event.events & (EPOLLERR | EPOLLHUP)) {
      running_ = false;
      continue;
    }

    // Timeouts are fatal since we specified an infinite timeout already.
    // Likewise, > 1 is not possible since we waited for one result.
    if (epoll_result != 1) {
      running_ = false;
      continue;
    }

    if (event.data.fd == timer_fd_.get()) {
      OnEventFired();
    }
  }
}

// |fml::MessageLoopImpl|
void MessageLoopOscortex::Terminate() {
  running_ = false;
  WakeUp(fml::TimePoint::Now());
}

// |fml::MessageLoopImpl|
void MessageLoopOscortex::WakeUp(fml::TimePoint time_point) {
  bool result = TimerRearm(timer_fd_.get(), time_point);
  (void)result;
  FML_DCHECK(result);
}

void MessageLoopOscortex::OnEventFired() {
  if (TimerDrain(timer_fd_.get())) {
    RunExpiredTasksNow();
  }
}

}  // namespace fml
