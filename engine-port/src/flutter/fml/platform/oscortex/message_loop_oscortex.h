// Copyright 2013 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// OSCortex native port: the fml message loop. Selected when FML_OS_OSCORTEX is
// defined (see flutter/fml/build_config.h, gated on the FLUTTER_OSCORTEX GN
// define). Currently mirrors the linux epoll+timerfd loop — the same mechanism
// whose hand-emulated equivalent livelocks rendering today; it will be diverged
// to OSCortex's native wait/timer primitives so the wakeup path is provably
// correct. See docs/native-engine-port.md.

#ifndef FLUTTER_FML_PLATFORM_OSCORTEX_MESSAGE_LOOP_OSCORTEX_H_
#define FLUTTER_FML_PLATFORM_OSCORTEX_MESSAGE_LOOP_OSCORTEX_H_

#include <atomic>

#include "flutter/fml/macros.h"
#include "flutter/fml/message_loop_impl.h"
#include "flutter/fml/unique_fd.h"

namespace fml {

class MessageLoopOscortex : public MessageLoopImpl {
 private:
  fml::UniqueFD epoll_fd_;
  fml::UniqueFD timer_fd_;
  bool running_ = false;

  MessageLoopOscortex();

  ~MessageLoopOscortex() override;

  // |fml::MessageLoopImpl|
  void Run() override;

  // |fml::MessageLoopImpl|
  void Terminate() override;

  // |fml::MessageLoopImpl|
  void WakeUp(fml::TimePoint time_point) override;

  void OnEventFired();

  bool AddOrRemoveTimerSource(bool add);

  FML_FRIEND_MAKE_REF_COUNTED(MessageLoopOscortex);
  FML_FRIEND_REF_COUNTED_THREAD_SAFE(MessageLoopOscortex);
  FML_DISALLOW_COPY_AND_ASSIGN(MessageLoopOscortex);
};

}  // namespace fml

#endif  // FLUTTER_FML_PLATFORM_OSCORTEX_MESSAGE_LOOP_OSCORTEX_H_
