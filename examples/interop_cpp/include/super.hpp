/**
 * Super C++20 header-only RAII wrapper over super.h.
 *
 *   super::Engine engine;
 *   engine.bind("host", "apply_damage", [](Entity& e, float dmg) { e.hp -= dmg; });
 *   engine.bind_span("entities", std::span<Entity>(entities));
 *   auto res = engine.call<float>("combat", "compute_dps", player, 1.5f);
 */
#pragma once

#include "super.h"

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <functional>
#include <memory>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <utility>
#include <vector>

namespace super {

namespace detail {

template <typename T>
struct always_false : std::false_type {};

template <typename T>
constexpr const char *type_token() {
  if constexpr (std::is_same_v<std::remove_cvref_t<T>, void>) {
    return "void";
  } else if constexpr (std::is_same_v<std::remove_cvref_t<T>, bool>) {
    return "bool";
  } else if constexpr (std::is_same_v<std::remove_cvref_t<T>, std::int32_t> ||
                       std::is_same_v<std::remove_cvref_t<T>, int>) {
    return "i32";
  } else if constexpr (std::is_same_v<std::remove_cvref_t<T>, std::int64_t> ||
                       std::is_same_v<std::remove_cvref_t<T>, long> ||
                       std::is_same_v<std::remove_cvref_t<T>, long long>) {
    return "i64";
  } else if constexpr (std::is_same_v<std::remove_cvref_t<T>, float>) {
    return "f32";
  } else if constexpr (std::is_same_v<std::remove_cvref_t<T>, double>) {
    return "f64";
  } else if constexpr (std::is_pointer_v<std::remove_cvref_t<T>>) {
    return "ptr";
  } else if constexpr (std::is_reference_v<T> || std::is_class_v<std::remove_cvref_t<T>>) {
    // Struct-by-reference / by-value: encode size for packed ABI.
    return nullptr; // handled specially
  } else {
    static_assert(always_false<T>::value, "unsupported Super FFI type");
    return "i64";
  }
}

template <typename T>
std::string type_token_str() {
  using U = std::remove_cvref_t<T>;
  if constexpr (std::is_reference_v<T> ||
                (std::is_class_v<U> && !std::is_same_v<U, std::string>)) {
    return std::string("S:") + std::to_string(sizeof(U));
  } else {
    const char *t = type_token<T>();
    return t ? std::string(t) : std::string("i64");
  }
}

template <typename... Args>
std::string make_arg_sig() {
  std::string out;
  (
      [&] {
        auto s = type_token_str<Args>();
        if (!out.empty())
          out += ',';
        out += s;
      }(),
      ...);
  return out;
}

template <typename R, typename... Args>
std::string make_sig() {
  auto args = make_arg_sig<Args...>();
  auto ret = type_token_str<R>();
  if (args.empty())
    return std::string("->") + ret;
  return args + "->" + ret;
}

template <typename T>
constexpr std::size_t aligned_size() {
  constexpr std::size_t a = alignof(std::remove_cvref_t<T>);
  constexpr std::size_t s = sizeof(std::remove_cvref_t<T>);
  return (s + a - 1) & ~(a - 1);
}

template <typename... Args>
constexpr std::size_t args_bytes() {
  return (aligned_size<Args>() + ... + 0);
}

template <typename T>
void write_slot(std::byte *&cursor, T &&value) {
  using U = std::remove_cvref_t<T>;
  auto addr = reinterpret_cast<std::uintptr_t>(cursor);
  auto align = alignof(U);
  addr = (addr + align - 1) & ~(align - 1);
  cursor = reinterpret_cast<std::byte *>(addr);
  if constexpr (std::is_reference_v<T>) {
    // Pass address of referenced object as struct blob / pointer hybrid:
    // copy the object bytes into the args buffer.
    std::memcpy(cursor, reinterpret_cast<const void *>(std::addressof(value)), sizeof(U));
  } else {
    std::memcpy(cursor, reinterpret_cast<const void *>(std::addressof(value)), sizeof(U));
  }
  cursor += aligned_size<U>();
}

template <typename T>
T read_slot(const std::byte *&cursor) {
  using U = std::remove_cvref_t<T>;
  auto addr = reinterpret_cast<std::uintptr_t>(cursor);
  auto align = alignof(U);
  addr = (addr + align - 1) & ~(align - 1);
  cursor = reinterpret_cast<const std::byte *>(addr);
  U value{};
  std::memcpy(&value, cursor, sizeof(U));
  cursor += aligned_size<U>();
  return value;
}

template <typename R, typename... Args>
struct Trampoline;

template <typename R, typename... Args>
struct Trampoline {
  using Fn = std::function<R(Args...)>;
  static void invoke(void *user_data, void *args_buf, void *ret_buf) {
    auto *fn = static_cast<Fn *>(user_data);
    const std::byte *cursor = static_cast<std::byte *>(args_buf);
    std::tuple<std::remove_cvref_t<Args>...> tup;
    auto fill = [&]<std::size_t... I>(std::index_sequence<I...>) {
      ((std::get<I>(tup) = read_slot<std::remove_cvref_t<Args>>(cursor)), ...);
    };
    fill(std::index_sequence_for<Args...>{});
    if constexpr (std::is_void_v<R>) {
      std::apply([&](auto &... xs) { (*fn)(xs...); }, tup);
      (void)ret_buf;
    } else {
      R result = std::apply([&](auto &... xs) { return (*fn)(xs...); }, tup);
      if (ret_buf) {
        std::memcpy(ret_buf, &result, sizeof(R));
      }
    }
  }
};

} // namespace detail

class Engine {
public:
  Engine() : eng_(super_engine_new(), &super_engine_free) {
    if (!eng_)
      throw std::runtime_error("super_engine_new failed");
  }

  Engine(const Engine &) = delete;
  Engine &operator=(const Engine &) = delete;
  Engine(Engine &&) noexcept = default;
  Engine &operator=(Engine &&) noexcept = default;

  SuperEngine *get() const { return eng_.get(); }

  void load(std::string_view path) {
    if (super_load_file(eng_.get(), std::string(path).c_str()) != 0)
      throw std::runtime_error(last_error());
  }

  void reload(std::string_view path) {
    if (super_reload_module(eng_.get(), std::string(path).c_str()) != 0)
      throw std::runtime_error(last_error());
  }

  std::string last_error() const {
    const char *e = super_last_error(eng_.get());
    return e ? std::string(e) : std::string{};
  }

  /** Bind a C++ callable; argument/return types are deduced into a C trampoline. */
  template <typename F>
  void bind(std::string_view module, std::string_view name, F &&f) {
    bind_impl(module, name, std::function(std::forward<F>(f)));
  }

  template <typename R, typename... Args>
  void bind(std::string_view module, std::string_view name, std::function<R(Args...)> f) {
    bind_impl(module, name, std::move(f));
  }

  template <typename T>
  void bind_span(std::string_view name, std::span<T> span, std::string_view layout = {}) {
    if (layout.empty()) {
      if (super_bind_slice(eng_.get(), std::string(name).c_str(), span.data(), span.size(),
                         sizeof(T)) != 0)
        throw std::runtime_error(last_error());
    } else {
      if (super_bind_slice_ex(eng_.get(), std::string(name).c_str(), span.data(), span.size(),
                            sizeof(T), std::string(layout).c_str()) != 0)
        throw std::runtime_error(last_error());
    }
  }

  template <typename R = void, typename... Args>
  R call(std::string_view module, std::string_view fn, Args &&... args) {
    constexpr std::size_t nbytes = detail::args_bytes<Args...>();
    std::vector<std::byte> buf(nbytes > 0 ? nbytes : 1);
    std::byte *cursor = buf.data();
    (detail::write_slot(cursor, std::forward<Args>(args)), ...);

    auto sig = detail::make_sig<R, Args...>();
    if constexpr (std::is_void_v<R>) {
      (void)cursor;
      if (super_call_sig(eng_.get(), std::string(module).c_str(), std::string(fn).c_str(),
                       buf.data(), nullptr, sig.c_str()) != 0) {
        throw std::runtime_error(last_error());
      }
    } else {
      alignas(R) std::byte ret_store[sizeof(R)]{};
      if (super_call_sig(eng_.get(), std::string(module).c_str(), std::string(fn).c_str(),
                       buf.data(), ret_store, sig.c_str()) != 0) {
        throw std::runtime_error(last_error());
      }
      R out{};
      std::memcpy(&out, ret_store, sizeof(R));
      return out;
    }
  }

private:
  template <typename R, typename... Args>
  void bind_impl(std::string_view module, std::string_view name,
                 std::function<R(Args...)> f) {
    auto *heap = new std::function<R(Args...)>(std::move(f));
    held_.emplace_back(heap, [](void *p) {
      delete static_cast<std::function<R(Args...)> *>(p);
    });
    auto sig = detail::make_sig<R, Args...>();
    if (super_bind_fn(eng_.get(), std::string(module).c_str(), std::string(name).c_str(),
                    &detail::Trampoline<R, Args...>::invoke, heap, sig.c_str()) != 0) {
      throw std::runtime_error(last_error());
    }
  }

  using Deleter = void (*)(SuperEngine *);
  std::unique_ptr<SuperEngine, Deleter> eng_;
  std::vector<std::unique_ptr<void, void (*)(void *)>> held_;
};

} // namespace super
