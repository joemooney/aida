declare module 'json-logic-js' {
  function apply(logic: unknown, data: unknown): unknown;
  function add_operation(name: string, fn: (...args: unknown[]) => unknown): void;
  export { apply, add_operation };
}
