# Rust 数据流分析器 C 语言支持设计

## 背景

当前分析器只支持 Python。它的后续处理链路已经基本做到语言无关：CLI 负责驱动源码发现，
前端负责把源码降到共享 IR，分析层负责计算 CFG 和数据流事实，报告层负责输出 HTML、
JSON/CSV、DOT 以及可选 SVG。

下一步要增加 C 语言支持，并且让用户可见的输出与 Python 保持一致。首个真实验收对象是
`/mnt/d/repos/arcs_mini/modules/lschat/tests`。这棵目录下有多个基于 CMake 的 C 测试应用，
依赖真实编译参数、本地头文件、系统头文件、宏和条件编译，因此不能走手写 include/define
的简化模式。

## 已确认需求

- 增加 `lang = "c"` 支持，与现有 Python 并存。
- 保持与 Python 一致的报告和导出布局：
  - 静态 HTML 报告
  - `analysis-cache.json`
  - CSV 导出
  - DOT 图
  - 在 Graphviz `dot` 可用时输出可选 SVG
- 为 C 提供与 Python 对齐的核心分析能力：
  - reaching definitions
  - def-use 边
  - variable-dependency 边
  - 每个函数的基础 CFG
  - 简单跨函数摘要传播
- 保持 `paths` 流程不变：读取 `analysis-cache.json` 并输出 `path-query.json`。
- 支持分析范围配置。
- 对 C 必须使用真实编译上下文，而不是手写 include/define 列表。
- 分析器必须在内部从输入树中的 CMake 工程生成 `compile_commands.json`，不能要求用户手动提供。
- 实现完成后需要用 `/mnt/d/repos/arcs_mini/modules/lschat/tests` 做验证。

## 非目标

- 不实现编译器级别的完整 C 语义分析。
- 不实现完整 points-to 或 alias analysis。
- 不完整建模内联汇编、链接期符号重写、`volatile` 副作用或复杂预处理元编程语义。
- 不要求用户学习单独的“生成编译数据库”命令；生成编译数据库属于 `analyze --lang c` 的一部分。
- 不因增加 C 支持而改变 Python 输出 schema 或破坏 Python 现有行为。

## 用户使用方式

### 分析

C 的主流程仍然使用现有 `analyze` 子命令：

```text
data-flow-analyzer analyze --lang c --input /mnt/d/repos/arcs_mini/modules/lschat/tests --out /tmp/lschat-report
```

分析器内部会执行以下步骤：

1. 在输入目录下发现基于 CMake 的测试应用
2. 对每个发现的工程执行 CMake configure，并启用 `CMAKE_EXPORT_COMPILE_COMMANDS=ON`
3. 收集并合并所有生成的 compile commands
4. 使用真实编译上下文对 translation unit 做预处理
5. 将得到的 C 代码降到共享 IR
6. 运行现有分析与报告输出链路

### 路径查询

`paths` 子命令保持不变：

```text
data-flow-analyzer paths --input /tmp/lschat-report/data/analysis-cache.json --function lsc_test::reset_host_transport_mocks
```

它会像 Python 一样，在 cache 文件旁边输出 `data/path-query.json`。

## CLI 和配置变更

### CLI

保留现有 `analyze` 与 `paths` 子命令，并给 `analyze` 增加 C 专属可选参数：

- `--build-root <path>`
  - 生成 CMake build tree 和 compile database 的目录
- `--cmake-arg <arg>`
  - 可重复，透传额外 CMake configure 参数
- `--keep-preprocessed`
  - 保留预处理后的 `.i` 文件和参数快照，便于调试

Python 运行时忽略这些参数。

### 配置

扩展 `AnalyzeConfig`，增加可选 C 专属字段：

- `build_root: Option<PathBuf>`
- `cmake_args: Vec<String>`
- `keep_preprocessed: bool`
- `c_project_globs: Vec<String>`

行为规则如下：

- `lang = "python"` 时忽略这些新字段。
- `lang = "c"` 时使用它们，同时仍支持共享字段，例如 `input`、`out`、`exclude`、
  `top_n`、`max_loop_unroll`、`max_paths`、`max_path_len`。
- `c_project_globs` 默认行为是递归搜索输入根目录下包含 `CMakeLists.txt` 的目录。

## 架构设计

### 总体流程

```text
CLI/config
  -> source/build discovery
      -> python: discover .py files
      -> c: discover CMake projects
  -> compile context stage
      -> python: none
      -> c: configure CMake projects and merge compile_commands.json
  -> frontend input preparation
      -> python: raw source files
      -> c: preprocessed translation units + source map
  -> language frontend
      -> lang::python or lang::c
  -> unified IR
  -> CFG
  -> reaching-definitions / def-use / var-deps
  -> summary propagation
  -> report/export writers
  -> path queries
```

### 新增 C 专属模块

- `src/lang/c.rs`
  - 基于 tree-sitter C 的解析与 IR lowering
- `src/cbuild.rs`
  - CMake 工程发现、configure 调度、compile database 合并
- `src/ccompile.rs`
  - compile command 解析、预处理调度、source-map 元数据

这样可以把 C 的构建上下文复杂度隔离在语言边界层，而不是污染共享分析逻辑。

## 编译数据库生成

### 工程发现

当 `lang = "c"` 时，分析器扫描输入树，寻找候选工程：

- 包含 `CMakeLists.txt` 的目录
- 可被 `c_project_globs` 进一步过滤
- 对 configure 之后没有可编译 C target 的嵌套辅助目录予以忽略

对于 LSChat 的验收树，预期将覆盖这些工程目录：

- `lsc_tests`
- `lsc_conn_tests`
- `session_request_tests`
- `session_text_tests`
- `session_voice_tests`
- `multi_session_tests`
- `session_voice_encode_tests`
- `stream_text/sync`

### Configure 阶段

对每个发现的工程执行 CMake configure，并带上：

- `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`
- 位于 `build_root` 或默认输出目录下的确定性 build 目录
- 用户通过 `--cmake-arg` 传入的额外参数

实现必须兼容 LSChat 测试树已有的 host-test 约定。这意味着分析器必须允许 CMake 消费
host coverage runner 同类变量，例如自定义 `LISA_BASE`，以及其他通过 `--cmake-arg`
传入的工程特定参数。

### 合并阶段

每个工程生成的 `compile_commands.json` 会被收集并归一化，最终合并成一个文件：

- `out/data/compile_commands.merged.json`

归一化规则：

- 对 `directory` 做 canonicalize
- 保留 `command` 或 `arguments` 中原始可恢复的编译命令信息
- 按 `(file, directory, normalized arguments)` 去重
- 按文件路径和参数摘要做确定性排序

这个合并后的文件就是 C 分析阶段的规范输入清单。

## 预处理策略

分析器不应直接解析原始 `.c` 文件，而应当把每条 compile command 转成预处理命令，在真实编译
上下文下展开头文件和宏。

### 预处理流程

对于每一条 compile command：

1. 解析原始 `command` 或 `arguments`
2. 提取源文件、include 路径、define、标准选项、forced include、工作目录
3. 以预处理模式调用编译器
4. 捕获预处理输出和紧凑的 source mapping 记录
5. 缓存这些结果供后续复用

### 需要保存的产物

对每个 translation unit，保存：

- 预处理后的源码文本
- 归一化参数快照
- 源文件标识与哈希
- 利用预处理行标记提取的轻量映射元数据，用于把预处理后的行映射回原始文件

这些产物应放在报告输出目录下的专用缓存区域，例如：

- `out/data/c-preprocessed/...`

如果 `keep_preprocessed` 为 false，分析器可以只保留确定性重跑和调试所需的最小缓存数据。

### 映射策略

IR 中的主 span 仍应尽量指回原始项目文件，只要 line marker 能支持这种映射。若某个节点来自外部
头文件或宏展开后无法精确映射到项目源码的合成区域，则应：

- 保留当前能得到的最佳文件与行号信息
- 在需要时将其标记为 external 或 synthetic
- 不伪造看似精确但并不可靠的本地 span

## C 前端 lowering

### 解析器

使用 `tree-sitter-c` 对预处理后的 C translation unit 做语法解析。

### 第一版支持的语法结构

第一版 C 前端需要把以下结构降到共享 IR：

- 函数定义和声明
- 局部变量定义
- 全局变量定义
- 赋值和复合赋值
- return
- expression statement
- 调用，包括直接调用和基础函数指针调用
- `if` / `else`
- `switch`
- `for`、`while`、`do while`
- `break`、`continue`、`goto`
- 一元和二元表达式
- 数组下标
- 结构体与联合体字段访问 `.` 和 `->`
- 指针解引用与取地址表达式

### IR 映射

复用现有 `Place` 家族，对 C 的映射规则如下：

- 普通局部/全局标量变量
  - `Local` 或 `Global`
- 结构体/联合体字段
  - `Attribute`
- 数组或指针下标访问
  - `Subscript`
- 闭包相关 place
  - C 中不使用
- 无法解析的外部状态、未知别名目标或危险回退
  - `External` 或 wildcard 风格 place key

具体 lowering 规则：

- `x = y`：在 `x` 上创建 definition，在 `y` 上创建 use
- `obj.field = value`：在 `Attribute(obj, field)` 上创建 definition，并在 `value` 上创建 use
- `ptr->field = value`：使用保守归一化后的 `ptr` base，降为 `Attribute(base, field)`
- `arr[i] = v`：在 `Subscript(arr, i)` 上创建 definition，并对 `arr`、`i`、`v` 创建 use
- `*ptr = v`：降为保守的解引用目标，而不是伪造一个精确 pointee

### 声明与跨文件解析

建立项目级 declaration/definition 索引，以支持：

- 头文件中的声明与 C 文件中的定义建立关联
- 直接函数调用跨 translation unit 解析到项目内定义
- unresolved 或 external 调用仍然生成 call record，并标记为 external

第一版只需要一个项目级的普通 C 符号表，不需要支持 C++ 风格重载。

## CFG 与数据流复用

现有共享 CFG 和分析层继续作为默认实现。C 前端的目标是产出这些共享层已经能消费的 IR 形状。

### CFG

每个函数需要产出：

- 一个 entry block
- 一个 exit block
- 顺序边
- 条件分支边
- `switch` case 边
- 循环回边
- `break`、`continue`、`goto` 的控制转移边

### Reaching Definitions 与 Def-Use

当前 reaching-definitions 引擎应尽量直接运行在 C 产生的 defs/uses 上；除非测试暴露明显缺口，
否则不优先分叉一套 C 专用实现。

### Variable Dependencies

当前 variable-dependency 分析应复用在 C 的 defs/uses 上，前提是 C 的 place 归一化规则保持稳定。

### 摘要传播

保留当前上下文不敏感的摘要模型：

- 函数参数进入 summary inputs
- return 涉及的 place 进入 summary returns
- 函数内观察到的写入进入 summary writes
- 调用点将参数 uses 传播到返回目标 defs 和写入影响

对于 unresolved 或 external C 调用，应保守标记 external effects，而不是假定它们是纯函数。

## 报告与导出

C 的报告输出必须与 Python 保持相同布局和 schema 风格：

- `index.html`
- `assets/report.css`
- `data/analysis-cache.json`
- `data/definitions.csv`
- `data/uses.csv`
- `data/def_use_edges.csv`
- `data/var_dependencies.csv`
- `data/function_summaries.csv`
- `data/parse_diagnostics.csv`
- `graphs/def_use_hotspots.dot`
- `graphs/variable_dependencies.dot`
- 在 Graphviz `dot` 可用时输出可选 SVG

允许在 `data/` 下额外写入 C 专属辅助文件，但它们不能替代共享的规范输出。

## 错误处理

### Configure / Compile Database 阶段

- 若未发现任何 CMake 工程，应报出清晰错误，说明输入树中没有可分析的 CMake 工程。
- 若某个工程 CMake configure 失败，应报告失败的工程路径以及使用的 configure 命令。
- 若至少有一个工程 configure 成功，则仅在剩余失败可被诊断并且不至于让结果失去意义时，才允许以部分结果继续；否则直接失败。

### 预处理阶段

- 若某个 translation unit 无法预处理，应在 diagnostics 中记录，并关联到源文件。
- 已成功的 translation unit 仍继续参与分析。
- 需要把 `fail_on_parse_error` 风格的“硬失败 / 部分输出”策略推广到 C，而不只针对 Python parse。

### 前端阶段

- 语法或 lowering 失败需要记录 diagnostics，并尽量指回原始文件路径。
- 只要还能产出足够 IR 让输出有用，就允许部分分析结果存在。

## 测试策略

### 仓库内单元与集成测试

新增针对以下内容的测试：

- compile command 发现与 merged database 生成
- 预处理命令归一化
- line marker source-map 提取
- C IR lowering：locals、globals、字段访问、下标、循环、调用、`goto`、未解析指针场景
- 代表性 C 片段上的 CFG 构建
- 代表性 C 片段上的 def-use 和 variable-dependency 生成
- 多个 C translation unit 间的 summary propagation
- 基于 C cache 的报告生成
- 基于 C cache 的 `paths` 查询

建议新增测试文件：

- `tests/c_compile_commands.rs`
- `tests/c_frontend.rs`
- `tests/c_integration_report.rs`

### 外部验收测试

新增一个 ignored integration test，直接跑真实 LSChat 目录：

- `/mnt/d/repos/arcs_mini/modules/lschat/tests`

这个测试应完成：

1. 对该目录执行 `analyze --lang c`
2. 断言 merged compile database 已输出
3. 断言共享报告产物存在
4. 对生成的 `analysis-cache.json` 执行 `paths`
5. 断言 `path-query.json` 存在

因为它依赖外部仓库和宿主机构建环境，所以默认标记为 ignored。

## 验收标准

当以下条件全部满足时，设计目标视为完成：

- Python 分析现有测试套件仍然通过。
- 分析器接受 `--lang c`。
- `analyze --lang c` 能发现并 configure
  `/mnt/d/repos/arcs_mini/modules/lschat/tests` 下的 LSChat 测试工程。
- 分析器输出 `out/data/compile_commands.merged.json`。
- 分析器能为 C 输出：
  - `analysis-cache.json`
  - CSV 导出
  - DOT 图
  - HTML 报告
  - 可选 SVG
- 分析器能对 C 代码完成至少基础 CFG、reaching definitions、def-use、
  variable dependencies 和跨函数摘要传播。
- `paths` 子命令可作用于 C 生成的 cache，并输出 `path-query.json`。
- 输出结构和 schema 与 Python 保持对齐，下游消费者不需要额外写一套 C 专用报告读取逻辑。

## 设计理由

这套设计尽量保持现有分析器架构稳定：

- IR、CFG、数据流、报告和路径查询层继续作为共享核心
- C 特有复杂度只放在它应在的位置：构建上下文、预处理和 AST lowering
- 用户只需要一个命令就能完成 C 分析，而不是手工跑多步流程

这种取舍可以先交付一个对真实测试工程足够实用的 C 第一版，而不必过早把整个项目重构成
编译器前端工具链。
