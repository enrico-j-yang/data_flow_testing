# Rust 数据流分析器设计

## 背景与目标

本项目要用 Rust 实现一个可扩展的数据流分析程序。第一版支持 Python，输入以
`D:\repos\asr_platform\app` 下的源码为首个真实验收对象；第二版支持 C，后续继续扩展更多语言。

第一版输出以 DOT/SVG 图和 HTML 报告为主，展示定义-使用关系、变量依赖关系、函数/模块级摘要图，以及指定文件或函数内的完整 def-clear CFG 路径。为支撑报告生成、局部路径查询和调试，工具内部会保留机器可读的 JSON/CSV 数据文件，但它们不是用户主要阅读入口。

## 已确认需求

- 使用 Rust 实现。
- 第一版支持 Python，后续支持 C 和更多语言。
- Python 解析使用 `tree-sitter-python`；第二版接入 `tree-sitter-c`。
- 架构采用统一 IR + CFG + 数据流框架，避免 Python-only 设计。
- 分析精度采用更严格版本：
  - 构建 CFG。
  - 做 reaching definitions。
  - 解析项目内 import。
  - 生成函数摘要。
  - 在调用点传播摘要依赖。
- “所有定义-使用路径”采用分层输出：
  - 全局默认输出摘要图和报告。
  - 对指定文件/函数展开完整 def-clear CFG 路径。
- 循环采用 CFG 回边 + 可配置展开上限，默认不无限枚举。
- 跨函数分析采用上下文不敏感函数摘要。
- 输出以 DOT/SVG + HTML 报告为主。
- 默认只渲染摘要 SVG；完整 def-use 图需要显式开启。

## 非目标

- 第一版不追求完整动态 Python 语义。
- 第一版不执行被分析代码。
- 第一版不保证精确建模 `eval`、`exec`、反射、动态 monkey patch、metaclass、descriptor、依赖注入和数据库字段隐式流。
- 第一版不做完整 points-to 分析；属性和下标访问采用保守别名策略。
- 第一版不默认渲染全量 def-use SVG，避免真实项目上图过大导致渲染或浏览卡死。
- 第一版不实现 C 支持，但核心设计必须让 C 前端可复用 IR、CFG、analysis 和 report 层。
- 第一版不解析第三方库源码；可读取本地 stub 或内置轻量模型，但未知第三方调用按外部副作用处理。

## 总体架构

```text
CLI
  -> configuration
      -> CLI args
      -> optional TOML config
  -> language frontend
      -> tree-sitter parser
      -> language-specific lowering
  -> unified IR
      -> symbols, scopes, definitions, uses, calls, captures
  -> CFG builder
      -> basic blocks, edges, loop back edges, exceptional edges
  -> dataflow analysis
      -> reaching definitions
      -> def-use edges
      -> variable dependencies
      -> function summaries
      -> call-site propagation
  -> path query engine
      -> def-clear CFG paths for selected functions/files
  -> graph/report output
      -> DOT
      -> SVG via Graphviz
      -> HTML report
      -> internal JSON/CSV data
```

### 模块划分

- `cli`
  - 负责命令行解析、配置合并、输出目录初始化、Graphviz 检测。
- `config`
  - 读取可选 TOML 配置文件。
  - CLI 参数覆盖配置文件。
- `lang`
  - 语言适配接口。
  - 第一版包含 `lang::python`。
  - 第二版增加 `lang::c`。
- `ir`
  - 定义语言无关 IR。
  - 包含 `Module`、`Function`、`ClassDef`、`Statement`、`Expression`、`Place`、`DefSite`、`UseSite`、`CallSite`、`CaptureSite`、`FreeVar`、`Scope`、`SourceSpan`。
- `cfg`
  - 构建模块级、函数级和方法级 CFG。
  - 包含基本块、边类型、路径标签、循环回边、入口/出口节点。
- `analysis`
  - 实现 reaching definitions、def-use 边、变量依赖边、别名近似、函数摘要和调用点传播。
- `paths`
  - 对指定文件/函数执行 def-clear 路径展开。
  - 使用 `--max-loop-unroll` 和 `--max-paths` 控制路径规模。
- `graph`
  - 生成 DOT。
  - 调用 Graphviz `dot` 渲染 SVG。
- `report`
  - 生成静态 HTML 报告、函数详情页、局部路径页和资源文件。

## 统一 IR

### SourceSpan

每个 IR 节点都记录源位置：

- 文件路径。
- 起始行/列。
- 结束行/列。
- 原始源码片段的短摘要。

内部路径统一归一化为正斜杠，例如 `app/routers/tests.py`。CLI 接受 Windows 反斜杠路径，但进入 IR、缓存、CSV、DOT 和 HTML 后统一使用正斜杠。

### 稳定 ID

所有对外可引用节点使用稳定 ID，不使用单纯遍历自增序号。

- `ModuleId`：`M_` + 归一化模块路径的短哈希。
- `FunctionId`：`F_` + `module_path::qualified_name::span` 的短哈希。
- `ScopeId`：`S_` + `module_path::qualified_name::scope_kind::span` 的短哈希。
- `DefId`：`D_` + `place_key::span::def_kind::ordinal_in_statement` 的短哈希。
- `UseId`：`U_` + `place_key::span::use_kind::ordinal_in_expression` 的短哈希。
- `BlockId`：`B_` + `function_id::block_index::span` 的短哈希。
- `EdgeId`：`E_` + `from_block::to_block::edge_kind::ordinal` 的短哈希。

哈希输入必须包含 schema 版本，避免未来字段语义变化导致旧缓存误用。发生极少数哈希冲突时追加确定性冲突序号。DOT 节点 ID 直接使用这些稳定 ID 的 Graphviz 安全形式，HTML anchor 和文件 slug 使用同一 ID 或其 URL 安全变体。

### Scope

Python 第一版支持以下作用域：

- module scope。
- class scope。
- function/method scope。
- closure/enclosing scope。
- comprehension scope。
- `global` 声明。
- `nonlocal` 声明。

作用域解析规则：

1. 函数内优先解析本地绑定。
2. 遇到 `nonlocal` 时向闭包作用域解析。
3. 遇到 `global` 时向模块作用域解析。
4. 推导式创建独立作用域，捕获外层引用。
5. 未解析符号标记为 unresolved 或 external，不伪装成精确结果。

### CaptureSite 和 FreeVar

闭包捕获必须在 IR 中显式表示。

- `FreeVar` 表示函数体内读取但不在当前函数本地绑定的变量。
- `CaptureSite` 表示 nested function 从外层作用域捕获变量。
- 捕获边记录：
  - 捕获变量名。
  - 捕获源作用域。
  - 捕获目标函数。
  - 捕获方式：read、write-via-nonlocal、global-read、global-write。
  - 对应的外层 `DefId` 集合。

函数摘要必须包含捕获输入和 `nonlocal` 写入输出，否则 nested function 的数据流会被截断。

### ClassDef 和继承信息

`ClassDef` 记录：

- 类名和限定名。
- base class 表达式。
- decorators。
- metaclass 关键字。
- 类体作用域。
- 方法列表。
- 类属性定义。
- 实例属性写入摘要。
- 解析后的项目内 base class 候选集合。

第一版采用保守 MRO 近似：

- 对项目内可解析 base class，按源码 base 列表构造局部 MRO 近似。
- 多继承冲突或动态 base 标记为 `mro_uncertain`。
- 方法解析先查当前类，再查可解析 base；多个候选目标按 union merge。
- `super().method(...)` 解析到当前类可解析 base 中的候选方法；无法确定时标记为 unresolved-super-call。
- 方法重写通过相同方法名和继承边记录 override 关系。

### Place

`Place` 是定义/使用和 kill 的核心抽象。

- `Local(name, scope_id)`。
- `Global(module_id, name)`。
- `Closure(scope_id, name)`。
- `Attribute(base_key, attr, sensitivity)`。
- `Subscript(base_key, index_key)`。
- `External(name)`。
- `Unknown(reason)`。

`base_key` 来自别名分析。无法确定对象身份时使用 `UnknownBase` 或 field-based fallback。

### DefSite

第一版 Python 定义点包括：

- `import` 和 `from ... import ...`。
- 函数定义。
- 类定义。
- 参数定义，包括普通参数、默认参数、可变参数和关键字参数。
- 闭包捕获绑定。
- 普通赋值。
- 解包赋值。
- 类型注解赋值。
- 增强赋值。
- `for`/`async for` 目标。
- `with ... as`/`async with ... as` 目标。
- `except ... as` 目标。
- `match` 绑定。
- 属性写入，例如 `self.x = value`。
- 下标写入，例如 `items[i] = value`。

### UseSite

第一版 Python 使用点包括：

- 变量读取。
- 属性读取。
- 下标读取。
- 函数调用的 callee 和实参。
- 默认参数表达式。
- 装饰器。
- 类型注解。
- 条件表达式。
- `await` 表达式。
- `return`、`yield`、`yield from`、`raise`。
- 上下文管理器表达式。
- 推导式。
- `match` subject、guard 和 case 模式中的值表达式。
- class pattern、value pattern 和 mapping pattern 中可执行或可解析的表达式。

### CallSite

调用点记录：

- callee 表达式。
- 位置参数和关键字参数。
- 接收返回值的目标定义点。
- 调用所在函数/模块。
- 可能 callee 集合。
- 解析状态：project-local、external、dynamic、unresolved、multi-target。

## 别名与字段敏感性策略

第一版采用保守、可解释的轻量别名策略，不实现完整 points-to 分析。

### 对象身份

- `self`/`cls` 在方法内识别为接收者对象：
  - `self.x` 归一化为 `InstanceField(ClassName, x)`。
  - `cls.x` 和类体内 `ClassName.x` 归一化为 `ClassField(ClassName, x)`。
- 显式构造调用 `obj = ClassName(...)` 在 callee 可解析为项目内类时生成 allocation site：
  - `Alloc(module_path, line, col, ClassName)`。
- 简单别名赋值 `a = obj` 在同一函数内传播 base_key。
- 参数对象默认为 `ParamObject(function_id, param_name)`。
- 无法解析的对象使用 `UnknownBase`。

### 字段敏感性

- 对已知 base_key 的属性访问采用 field-sensitive place，例如 `Attr(Alloc@L10, "x")`。
- 对未知 base_key 的属性访问采用 field-based fallback，例如 `Attr(*, "x")`。
- 写入 `Attr(*, "x")` 会 conservatively kill 或影响所有同名未知属性读取。
- 写入已知 `Attr(base, "x")` 不 kill 其他 base 的同名属性。
- 如果一个表达式可能有多个 base_key，读取和写入对候选集合取 union。

### 下标敏感性

- 常量字符串/整数下标保留 index_key，例如 `Subscript(base, "status")`。
- 动态下标降级为 `Subscript(base, *)`。
- 写入 `Subscript(base, *)` 会影响同一 base 下所有下标读取。
- 未知 base 的下标写入按保守 unknown-container side effect 处理。

## CFG 设计

CFG 以函数/方法为主要粒度，模块顶层也构建 CFG。

### 节点

- Entry。
- Exit。
- BasicBlock。
- SyntheticBlock。

`SyntheticBlock` 必须带结构化原因和可选 payload：

- `ExceptionDispatch`：异常分发。
- `FinallyJoin`：finally 汇合。
- `LoopSummary`：循环摘要。
- `PathTruncation`：路径枚举截断提示。
- `UnknownSideEffect`：外部调用或动态语义造成的保守副作用。
- `ParseErrorBoundary`：tree-sitter ERROR 节点或无法 lowering 的局部区域。

SyntheticBlock 默认不产生普通定义点；只有 `UnknownSideEffect` 可产生保守 kill/unknown def。

### 边类型

- 顺序边。
- 条件 true/false 边。
- 循环 body 边。
- 循环 normal-exit 边。
- 循环 else 边。
- 循环回边。
- `break` exit 边。
- `continue` back 边。
- `return` 边。
- `raise` 边。
- `try` -> `except` 边。
- `try` -> `else` 边。
- `try` -> `finally` 边。
- `with` 入口/出口边。
- `await` suspend/resume 边。
- `yield` suspend/resume 边。
- `match` case 边。

### Python 特有控制流

- `for-else` 和 `while-else`：
  - 循环条件自然结束进入 else 边。
  - `break` 直接进入循环出口，不经过 else。
  - `continue` 回到循环条件或迭代点。
- `await`：
  - `await expr` 读取 `expr`，生成 suspend/resume 边。
  - 若 awaited callee 可解析，应用其 async 函数摘要。
  - 未解析 await 记录外部异步副作用。
- generator：
  - `yield expr` 读取 `expr`，生成 yield-output 和 suspend/resume 边。
  - `yield from expr` 读取委托对象，并标记 delegate-yield dependency。
  - generator 摘要包含 `yield_values` 和 `resume_inputs`。
- `async for` 和 `async with`：
  - 作为普通 for/with CFG 的异步变体建模，同时在迭代器或上下文管理器表达式处加入 await-like external effect。

### 循环处理

- CFG 中保留真实回边。
- 全局摘要不展开无限路径。
- 局部路径查询按 `--max-loop-unroll` 展开。
- 默认 `--max-loop-unroll 2`，覆盖“首次迭代定义 -> 后续迭代使用”的常见路径。
- 达到展开上限时，在路径结果中标记 `truncated-by-loop-limit`。

## 数据流分析

### Reaching Definitions

分析以 CFG 基本块为单位，采用前向 may analysis。

- Domain：`Place -> Set<DefId>`。
- Meet 算子：对每个 `Place` 的 reaching def 集合取 union。
- Transfer：
  - `OUT[B] = GEN[B] union (IN[B] - KILL[B])`。
  - `KILL[B]` 是同一 canonical place 的旧定义集合；未知副作用可 kill 一组保守 place。
- 迭代算法：
  - 使用 worklist。
  - 初始化 Entry 的 `IN` 为空，模块/函数参数和外部输入作为 synthetic entry definitions。
  - CFG 边发生状态变化时重新入队后继。
  - 到 fixpoint 后，为每个 UseSite 查询其所在块内最近定义和块入口 reaching defs。

### Def-use 边

边记录：

- `DefId`。
- `UseId`。
- 变量/Place 名称。
- 源文件和作用域。
- def 源位置。
- use 源位置。
- 边类型：
  - local。
  - enclosing。
  - closure-capture。
  - global。
  - cross-module-global。
  - import。
  - parameter。
  - attribute。
  - subscript。
  - call-summary。
  - external-conservative。
  - unresolved。

### 变量依赖

变量依赖来自赋值、参数传递、返回值、闭包捕获和函数摘要：

- RHS 变量 -> LHS 定义。
- 实参 -> 形参。
- 捕获变量 -> nested function free var。
- 函数内部输入 -> 返回值。
- 函数内部输入 -> yield 值。
- 函数内部输入 -> 属性/全局写入。
- 调用摘要输出 -> 调用点接收变量。

### 函数摘要

每个函数/方法生成上下文不敏感摘要：

- 参数输入。
- 闭包捕获输入。
- 全局读取。
- 属性读取。
- 返回值依赖。
- yield 值依赖。
- `nonlocal` 写入。
- 全局写入。
- 属性写入。
- 异常依赖。
- 外部调用依赖。

调用点传播规则：

1. 解析 callee 候选集合。
2. 将实参依赖映射到每个候选函数的形参。
3. 应用每个候选函数摘要。
4. 多个候选目标的输出取 union，并把调用点标记为 multi-target。
5. 将返回值依赖映射到接收变量或表达式使用点。
6. 将被调函数的全局/属性写入映射回调用点可见 place。
7. 未解析/外部调用生成保守依赖：
   - 返回值依赖所有实参和 callee。
   - 对 escaped 对象、未知属性和已知 mutable 参数记录 unknown side effect。
   - 对外部库已知纯函数模型可跳过副作用。

### 调用图和递归

- 先建立 project-local 调用图。
- 对函数摘要按调用图 SCC 迭代到 fixpoint。
- 递归或互递归函数在同一 SCC 内 union merge 摘要。
- 超过迭代上限时输出 diagnostic，并保留当前保守摘要。

### 跨模块全局变量

- 模块级绑定归一化为 `Global(module_id, name)`。
- `from module import name` 让本地 import 绑定 alias 到源模块 `Global`。
- `import module` 让模块对象属性读取 `module.name` alias 到源模块 `Global`。
- 可解析跨模块写入 `module.name = value` 映射到源模块 `Global(module_id, name)`。
- 无法确定模块对象时按 external/dynamic 处理。
- 模块顶层执行顺序不做完整运行时模拟；报告中标记 import cycle 和 top-level side effect 风险。

## Import 解析

第一版解析项目内 import：

- 支持从输入根目录推断 Python 模块路径。
- 支持绝对 import，例如 `from app.x import y`。
- 支持相对 import，例如 `from .x import y`。
- 支持 `__init__.py` re-export：
  - 解析显式 import re-export。
  - 解析静态 `__all__ = [...]`。
  - `from package import *` 使用 `__all__`；没有 `__all__` 时导出非 `_` 前缀符号。
- 支持 namespace package 近似：
  - 没有 `__init__.py` 的目录仍可作为包路径。
  - 多个同名 namespace root 合并为候选模块集合。
- 第三方库：
  - 默认标记为 external，不分析源码。
  - 可通过 `--stub-path` 或配置文件提供 `.pyi`/简化 stub。
  - 可读取 `requirements.txt`、`pyproject.toml`、`setup.cfg` 仅用于外部依赖分类和报告，不自动安装或联网抓取。
  - 内置少量通用模型：pure function、constructor-like call、context manager、decorator pass-through。
- 动态 import 标记为 dynamic-import。

## 解析错误恢复

tree-sitter 可在存在语法错误时生成包含 ERROR 节点的 parse tree。第一版恢复策略：

- 文件可读取且 tree-sitter 返回 tree 时，继续 lowering 可解析区域。
- 遇到 ERROR 节点时创建 `ParseErrorBoundary` SyntheticBlock 和 diagnostic。
- 如果 ERROR 位于函数体局部语句，跳过该语句或子树，继续分析同一函数后续块。
- 如果 ERROR 破坏函数/类边界，跳过该函数/类，继续分析同文件其他顶层节点。
- 文件读取、编码或 tree-sitter 语言初始化失败时跳过整个文件。
- 所有错误写入 `parse_diagnostics.csv` 和 HTML 报告。
- `--fail-on-parse-error` 在报告生成后返回非 0。

## 路径查询

全局默认不枚举所有 CFG 路径，避免路径爆炸。

局部路径查询支持：

- 指定文件。
- 指定函数/方法。
- 指定 `DefId`。
- 指定 `UseId`。
- 指定变量名。

路径必须满足 def-clear：

- 从定义点到使用点可达。
- 中间没有对同一 canonical Place 的 kill/redefine。
- 循环按展开上限处理。
- 路径中记录每条 CFG 边标签。

路径展开算法：

- 在目标函数 CFG 上执行 bounded DFS。
- 状态包含当前 block、当前 reaching def、每条回边访问次数、路径长度。
- 超过 `--max-loop-unroll`、`--max-paths` 或 `--max-path-len` 时截断并记录原因。
- 默认只在指定函数内展开；跨函数路径通过 call-summary 边折叠展示。

路径结果包含：

- 路径 ID。
- 起始定义点。
- 终止使用点。
- 基本块序列。
- 源码位置序列。
- 分支标签。
- 循环展开次数。
- 是否被截断。

## CLI 设计

### 默认分析命令

```powershell
data-flow-analyzer analyze --lang python --input D:\repos\asr_platform\app --out D:\tmp\dataflow_report
```

行为：

- 递归扫描 `.py` 文件。
- 排除 `.venv`、`venv`、`__pycache__`、`.pytest_cache`、`site-packages`、`build`、`dist`。
- 生成默认 DOT/SVG 摘要图。
- 生成 HTML 报告。
- 生成内部 JSON/CSV 数据。

### 路径查询命令

```powershell
data-flow-analyzer paths --input D:\tmp\dataflow_report\data\analysis-cache.json --function app/routers/tests.py::create_test --max-loop-unroll 2
```

行为：

- 读取分析缓存。
- 对指定函数生成 CFG 图和 def-clear 路径图。
- 更新或新增对应函数详情页。

### 配置文件

支持 TOML 配置：

```powershell
data-flow-analyzer analyze --config dataflow.toml
```

示例：

```toml
lang = "python"
input = "D:/repos/asr_platform/app"
out = "D:/tmp/dataflow_report"
max_loop_unroll = 2
top_n = 100
emit_full_dot = false
render_full_svg = false
fail_on_parse_error = false
parallelism = "auto"
exclude = ["**/.venv/**", "**/__pycache__/**", "**/site-packages/**"]
stub_paths = ["D:/repos/data_flow_testing/stubs"]
```

CLI 参数优先级高于配置文件。

### 重要参数

- `--config <path>`：读取 TOML 配置文件。
- `--max-loop-unroll <n>`：循环展开上限，默认 2。
- `--max-paths <n>`：单个查询最多输出路径数，默认 1000。
- `--max-path-len <n>`：单条路径最大 block 数，默认 500。
- `--top-n <n>`：热点图和报告展示数量，默认 100。
- `--emit-full-dot`：输出完整 def-use DOT。
- `--render-full-svg`：渲染完整 def-use SVG。
- `--stub-path <path>`：加载本地 stub。
- `--fail-on-parse-error`：解析失败时报告生成后返回非 0。
- `--no-open`：只生成报告，不尝试打开浏览器。

## 输出目录

```text
report/
  index.html
  assets/
    report.css
    report.js
  graphs/
    module_dependency.dot
    module_dependency.svg
    function_dependency.dot
    function_dependency.svg
    var_dependency.dot
    var_dependency.svg
    def_use_hotspots.dot
    def_use_hotspots.svg
  functions/
    <safe-function-id>.html
    <safe-function-id>.cfg.dot
    <safe-function-id>.cfg.svg
    <safe-function-id>.paths.dot
    <safe-function-id>.paths.svg
  data/
    analysis-cache.json
    definitions.csv
    uses.csv
    def_use_edges.csv
    var_dependencies.csv
    function_summaries.csv
    parse_diagnostics.csv
```

## analysis-cache.json schema

`analysis-cache.json` 是 `paths` 子命令的接口契约。第一版使用版本化 JSON。

顶层结构：

```json
{
  "schema_version": 1,
  "tool_version": "0.1.0",
  "generated_at": "2026-05-27T00:00:00Z",
  "input": {
    "lang": "python",
    "root": "D:/repos/asr_platform/app",
    "normalized_root": "D:/repos/asr_platform/app"
  },
  "config": {},
  "files": [],
  "modules": [],
  "scopes": [],
  "classes": [],
  "functions": [],
  "definitions": [],
  "uses": [],
  "captures": [],
  "calls": [],
  "cfgs": [],
  "def_use_edges": [],
  "var_dependency_edges": [],
  "function_summaries": [],
  "diagnostics": [],
  "graph_index": []
}
```

核心对象字段：

- `files[]`：`file_id`、`path`、`hash`、`line_count`、`parse_status`。
- `modules[]`：`module_id`、`file_id`、`module_name`、`exports`、`imports`。
- `scopes[]`：`scope_id`、`scope_kind`、`parent_scope_id`、`owner_id`、`span`。
- `classes[]`：`class_id`、`module_id`、`qualified_name`、`base_exprs`、`resolved_bases`、`mro_status`、`methods`、`span`。
- `functions[]`：`function_id`、`module_id`、`class_id`、`qualified_name`、`kind`、`params`、`scope_id`、`span`。
- `definitions[]`：`def_id`、`place`、`def_kind`、`scope_id`、`function_id`、`span`、`expr`、`deps`。
- `uses[]`：`use_id`、`place`、`use_kind`、`scope_id`、`function_id`、`span`、`context`。
- `captures[]`：`capture_id`、`source_scope_id`、`target_function_id`、`place`、`mode`、`span`。
- `calls[]`：`call_id`、`function_id`、`callee_expr`、`candidate_function_ids`、`resolution`、`arg_use_ids`、`return_target_def_id`、`span`。
- `cfgs[]`：`function_id`、`blocks`、`edges`、`entry_block_id`、`exit_block_id`。
- `def_use_edges[]`：`edge_id`、`def_id`、`use_id`、`place`、`edge_kind`、`path_summary`。
- `var_dependency_edges[]`：`edge_id`、`source_place`、`target_place`、`source_id`、`target_id`、`dep_kind`、`span`。
- `function_summaries[]`：`function_id`、`inputs`、`returns`、`yields`、`writes`、`raises`、`external_effects`、`fixpoint_status`.
- `diagnostics[]`：`diagnostic_id`、`severity`、`kind`、`message`、`file_id`、`span`。
- `graph_index[]`：`graph_id`、`kind`、`dot_path`、`svg_path`、`html_path`。

任何 schema-breaking 变化必须增加 `schema_version`。

## CSV schema

CSV 使用 UTF-8、逗号分隔、首行为表头。列表字段用分号分隔，复杂字段存 JSON 字符串。

- `definitions.csv`
  - `def_id,file,path,line,col,end_line,end_col,scope_id,function_id,place,def_kind,expr,deps`
- `uses.csv`
  - `use_id,file,path,line,col,end_line,end_col,scope_id,function_id,place,use_kind,context`
- `def_use_edges.csv`
  - `edge_id,def_id,use_id,place,edge_kind,def_file,def_line,use_file,use_line,path_summary`
- `var_dependencies.csv`
  - `edge_id,source_place,target_place,source_id,target_id,dep_kind,file,line,context`
- `function_summaries.csv`
  - `function_id,qualified_name,file,line,inputs,returns,yields,writes,raises,external_effects,fixpoint_status`
- `parse_diagnostics.csv`
  - `diagnostic_id,severity,kind,file,line,col,end_line,end_col,message`

## HTML 报告

### 首页

`index.html` 展示：

- 项目概览。
- 文件数。
- 函数数。
- 类数量。
- 定义点数量。
- 使用点数量。
- def-use 边数量。
- 变量依赖边数量。
- 解析错误数量。
- 模块依赖图。
- 函数依赖图。
- 变量依赖图。
- def-use 热点图。
- 热点文件/函数/变量。
- 未解析符号。
- 动态语义风险。
- 路径爆炸或循环截断提示。

### 文件视图

文件视图展示：

- 文件内定义/使用统计。
- 函数列表。
- 类列表。
- import 关系。
- 未解析符号。
- 指向函数详情页的链接。

### 函数视图

函数视图展示：

- CFG SVG。
- def-use 列表。
- 变量依赖表。
- 调用摘要。
- 闭包捕获。
- 循环、异步和异常流提示。
- 路径查询结果入口。

### 路径详情

路径详情展示：

- definition -> use 的 def-clear 路径。
- 每条路径的源码位置。
- CFG 边标签。
- 循环展开次数。
- 中间重定义检查结果。
- 截断原因。

## Graphviz 策略

- Rust 始终生成 DOT。
- 如果发现 `dot` 可用，默认渲染摘要 SVG。
- 如果未发现 `dot`，HTML 报告仍生成，并提示手动运行：

```powershell
dot -Tsvg input.dot -o output.svg
```

- 完整 def-use 图默认只生成 DOT。
- 只有显式传入 `--render-full-svg` 才渲染完整 SVG。

## 并行与性能

第一版使用 `rayon` 或等价线程池并行处理可独立阶段。

- 文件扫描、读取、tree-sitter 解析和 per-file lowering 可并行。
- CFG 构建可按函数并行。
- 单函数 reaching definitions 可并行。
- 函数摘要在调用图 SCC 之间可并行，SCC 内迭代。
- 报告表格生成可并行聚合；Graphviz 渲染按图并行但限制并发数。

目标性能以 `D:\repos\asr_platform\app` 约 1.5 万行 Python 为基准：

- 扫描 + 解析 + lowering：秒级，目标小于 5 秒。
- CFG + reaching definitions + 函数摘要：十秒级，目标小于 30 秒。
- 默认 HTML + 摘要 DOT/SVG：目标小于 15 秒，不包含异常缓慢的 Graphviz 情况。
- 完整 def-use SVG 和大规模路径展开不纳入默认性能目标。

主要瓶颈预计来自：

- 函数摘要 SCC fixpoint。
- 大函数路径展开。
- Graphviz 渲染大型图。

## 测试策略

### 单元测试

覆盖：

- Python 语法前端：
  - 赋值。
  - 解包。
  - 参数。
  - 闭包。
  - 类/继承/方法。
  - import。
  - match。
  - with。
  - except。
  - async/await。
  - generator。
  - 推导式。
- CFG：
  - 顺序。
  - if/else。
  - loop。
  - for-else/while-else。
  - break/continue。
  - return。
  - await/yield。
  - try/except/finally。
- 数据流：
  - reaching definitions。
  - def-clear path。
  - 变量依赖。
  - 别名和属性近似。
  - 函数摘要。
  - 调用点传播。
  - 跨模块全局变量。
- 输出：
  - DOT 转义。
  - 稳定节点 ID。
  - JSON schema。
  - CSV schema。
  - HTML 链接。
  - SVG 引用。

### 黄金样例测试

建议添加 fixtures：

- `fixtures/python_basic/`：基础定义/使用。
- `fixtures/python_control_flow/`：分支、循环、异常。
- `fixtures/python_calls/`：函数摘要和跨函数传播。
- `fixtures/python_imports/`：项目内 import、`__all__`、re-export 和外部 import。
- `fixtures/python_closure/`：闭包捕获、`nonlocal` 和推导式作用域。
- `fixtures/python_classes/`：继承、override、`super()` 和属性写入。
- `fixtures/python_async_generator/`：`await`、`async for`、`yield` 和 `yield from`。
- `fixtures/python_aliasing/`：简单别名、属性、下标和未知 base fallback。

每个 fixture 对应 expected 摘要，保证输出稳定。

### 真实项目验收

对 `D:\repos\asr_platform\app` 运行完整分析。

验收标准：

- 无 panic。
- 解析失败以诊断形式进入报告。
- 默认 SVG 可打开。
- HTML 有文件/函数索引。
- 能对指定函数生成路径详情。
- 输出数量级与现有 Python 原型结果进行 sanity check。

## 性能与规模控制

- 默认只渲染摘要图。
- 全量 def-use 数据保存到内部数据文件和报告表格。
- 完整 SVG 渲染需要显式开启。
- 路径查询限制循环展开、路径数量和路径长度。
- 报告展示 top-N，完整数据通过函数详情或内部数据文件访问。

## 实现里程碑

1. Rust 工程骨架、配置文件和 CLI。
2. Python 前端、解析错误恢复与统一 IR。
3. 作用域、闭包捕获、类继承和 import 解析。
4. 别名与 Place 归一化。
5. CFG 构建。
6. Reaching definitions 和 def-use 边。
7. 函数摘要、调用图 SCC 和调用点传播。
8. 局部路径展开。
9. DOT/SVG 和 HTML 报告。
10. JSON/CSV schema 稳定化。
11. 在 `D:\repos\asr_platform\app` 上验证并调优。

## 风险与应对

- Python 动态语义导致不精确。
  - 应对：显式标记 unresolved/external/dynamic，不输出虚假精确结果。
- 别名分析不完整导致属性边过多或过少。
  - 应对：已知 base 使用 field-sensitive place，未知 base 使用 field-based fallback，并在报告中展示不确定性。
- 完整路径枚举可能爆炸。
  - 应对：默认分层输出，局部路径查询限制循环展开、路径数量和路径长度。
- 完整图过大。
  - 应对：默认摘要图，完整 SVG 需显式开启。
- tree-sitter CST 到 IR 的 lowering 复杂。
  - 应对：先覆盖高频语法，使用 fixtures 做回归。
- 跨语言扩展时 Python 特性污染核心。
  - 应对：核心 IR/CFG/analysis 保持语言无关，Python 特性留在前端和语言扩展字段中。
- 第三方库副作用建模不足。
  - 应对：默认 external conservative，允许本地 stub 和轻量内置模型逐步补充。

## Review 修订记录

本版根据 review 补充了以下设计决策：

- IR 增加 `CaptureSite`、`FreeVar`、`ClassDef` 继承信息和 `Place` 抽象。
- 明确闭包捕获、类继承、`super()`、async/generator、`for-else`/`while-else` 的 CFG 语义。
- 明确别名与字段敏感性策略。
- 明确 reaching definitions 算法、meet 算子、worklist 和 call-summary merge。
- 明确跨模块全局变量传播。
- 补充 import re-export、`__all__`、namespace package、第三方 stub 策略。
- 补充 `analysis-cache.json` 和 CSV schema。
- 补充稳定 ID 和 DOT 节点 ID 策略。
- 增加 `--config`、`--stub-path`、`--max-paths`、`--max-path-len`。
- 默认 `--max-loop-unroll` 改为 2。
- 补充解析错误恢复、并行处理和更具体的性能目标。

## 自检结论

- 无 TBD/TODO 占位。
- 架构、输出和测试策略与已确认需求一致。
- 已补齐闭包、继承、别名、异步/生成器和 Python 循环 else 的关键设计。
- 已定义 `analysis-cache.json`、CSV 和稳定 ID 契约。
- 明确了第一版范围和非目标。
- 明确了动态语义、路径爆炸和图规模的处理方式。
- 当前设计足够进入实现计划阶段。
