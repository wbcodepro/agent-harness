class Emitter {
  constructor() {
    this.listeners = {};
  }

  on(event, fn) {
    (this.listeners[event] = this.listeners[event] || []).push(fn);
    return this;
  }

  once(event, fn) {
    const wrapper = (...args) => {
      fn(...args);
      this.off(event, wrapper);
    };
    return this.on(event, wrapper);
  }

  off(event, fn) {
    const list = this.listeners[event] || [];
    const index = list.indexOf(fn);
    if (index >= 0) list.splice(index, 1);
    return this;
  }

  emit(event, ...args) {
    const list = this.listeners[event] || [];
    for (let i = 0; i < list.length; i++) {
      list[i](...args);
    }
    return list.length > 0;
  }
}

module.exports = { Emitter };
