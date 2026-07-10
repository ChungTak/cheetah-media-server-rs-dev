#!/usr/bin/env python3
"""Add English + Chinese doc comments to Rust public items.

Strategy:
- For items with no doc comment, generate a concise English + Chinese comment
  block and insert it immediately before the first non-attribute line of the item.
- For items with an existing English-only doc comment, append a Chinese translation
  as a new `///` line after the existing block.
- For items with an existing Chinese-only doc comment, append an English translation.
- Items with both languages are left unchanged.
"""

import re
from pathlib import Path
from typing import Optional

from tree_sitter import Language, Parser, Node
import tree_sitter_rust

# Core files that should be manually curated; the script will still append
# a missing translation to existing English/Chinese doc comments, but it will
# not generate generic comments for items that have no comments.
CORE_FILES = {
    "crates/foundation/cheetah-codec/src/frame.rs",
    "crates/foundation/cheetah-codec/src/track.rs",
    "crates/sdk/cheetah-sdk/src/module.rs",
    "crates/sdk/cheetah-sdk/src/stream.rs",
    "crates/sdk/cheetah-sdk/src/ids.rs",
    "crates/runtime/cheetah-runtime-api/src/lib.rs",
    "crates/system/cheetah-engine/src/engine.rs",
    "apps/cheetah-server/src/main.rs",
}

# Simple phrase map from English to Chinese for generated descriptions.
# Code identifiers (backtick-wrapped) are kept as-is, so the map only translates
# natural-language function words.
EN_ZH = {
    "data structure": "数据结构",
    "enumeration": "枚举",
    "variant": "变体",
    "field": "字段",
    "function": "函数",
    "method": "方法",
    "trait": " trait",
    "constant": "常量",
    "static variable": "静态变量",
    "type alias": "类型别名",
    "module": "模块",
    "Creates": "创建",
    "creates": "创建",
    "Returns": "返回",
    "returns": "返回",
    "Constructs": "构造",
    "constructs": "构造",
    "Builds": "构建",
    "builds": "构建",
    "Computes": "计算",
    "computes": "计算",
    "Validates": "验证",
    "validates": "验证",
    "Checks": "检查",
    "checks": "检查",
    "Initializes": "初始化",
    "initializes": "初始化",
    "Starts": "启动",
    "starts": "启动",
    "Stops": "停止",
    "stops": "停止",
    "Handles": "处理",
    "handles": "处理",
    "Processes": "处理",
    "processes": "处理",
    "Encodes": "编码",
    "encodes": "编码",
    "Decodes": "解码",
    "decodes": "解码",
    "Parses": "解析",
    "parses": "解析",
    "Serializes": "序列化",
    "serializes": "序列化",
    "Deserializes": "反序列化",
    "deserializes": "反序列化",
    "Sends": "发送",
    "sends": "发送",
    "Receives": "接收",
    "receives": "接收",
    "Reads": "读取",
    "reads": "读取",
    "Writes": "写入",
    "writes": "写入",
    "Opens": "打开",
    "opens": "打开",
    "Closes": "关闭",
    "closes": "关闭",
    "Updates": "更新",
    "updates": "更新",
    "Removes": "移除",
    "removes": "移除",
    "Adds": "添加",
    "adds": "添加",
    "Inserts": "插入",
    "inserts": "插入",
    "Deletes": "删除",
    "deletes": "删除",
    "Clears": "清空",
    "clears": "清空",
    "Resets": "重置",
    "resets": "重置",
    "a new": "新的",
    "an": "一个",
    "a": "一个",
    "the": "",
    "of": "的",
    "with": "带有",
    "for": "用于",
    "to": "为",
    "from": "来自",
    "and": "和",
    "or": "或",
    "if": "如果",
    "True": "真",
    "true": "真",
    "False": "假",
    "false": "假",
    "is true": "为真",
    "is set": "被设置",
    "value": "值",
    "representation": "表示",
    "input": "输入",
    "output": "输出",
    "instance": "实例",
    "type": "类型",
    "parameter": "参数",
    "from input": "从输入",
    "set": "设置",
}


def get_node_text(node: Node, source: bytes) -> str:
    return source[node.start_byte:node.end_byte].decode("utf-8", errors="replace")


def is_pub(node: Node, source: bytes) -> bool:
    for child in node.children:
        if child.type == "visibility_modifier":
            text = get_node_text(child, source)
            if text.startswith("pub"):
                return True
    return False


def first_child_by_type(node: Node, type_name: str) -> Optional[Node]:
    for child in node.children:
        if child.type == type_name:
            return child
    return None


def find_name(node: Node, source: bytes) -> Optional[str]:
    for child in node.children:
        if child.type in ("identifier", "type_identifier", "field_identifier"):
            return get_node_text(child, source)
    return None


def find_field_type(node: Node, source: bytes) -> Optional[str]:
    field = node if node.type == "field_declaration" else first_child_by_type(node, "field_declaration")
    if field is None:
        return None
    for child in field.children:
        if child.type in ("type_identifier", "primitive_type", "reference_type", "array_type", "tuple_type"):
            return get_node_text(child, source)
    return None


def line_text(line: int, source: bytes) -> str:
    start = line_start_byte(line, source)
    end = line_start_byte(line + 1, source)
    return source[start:end].decode("utf-8", errors="replace")


def line_start_byte(line: int, source: bytes) -> int:
    """Return the byte offset of the start of the 0-indexed `line`."""
    if line <= 0:
        return 0
    count = 0
    idx = 0
    while count < line and idx < len(source):
        if source[idx] == ord("\n"):
            count += 1
        idx += 1
    return idx


def parse_doc_text(comments: list[str]) -> str:
    parts = []
    for c in comments:
        if c.startswith("///") and not c.startswith("////"):
            parts.append(c[3:].strip())
    return " ".join(parts)


def has_chinese(text: str) -> bool:
    return bool(re.search(r"[\u4e00-\u9fff]", text))


def translate_en_to_zh(english: str) -> str:
    """Rule-based translation of short English phrases to Chinese."""
    text = english
    # Translate whole phrases before individual words to avoid mangled Chinese.
    text = text.replace("field of type", "字段，类型为")
    text = re.sub(r"` of type `([^`]+)`", "类型为 `\\1`", text)
    text = text.replace("data structure", "数据结构")
    text = text.replace("static variable", "静态变量")
    text = text.replace("type alias", "类型别名")
    for en, zh in EN_ZH.items():
        text = re.sub(rf"\b{re.escape(en)}\b", zh, text)
    text = re.sub(r"\s+", " ", text).strip()
    return text


def translate_zh_to_en(chinese: str) -> str:
    """Translate a Chinese doc string to English by keeping identifiers and using a phrase map."""
    # We keep this simple and identity-like for the first pass; a phrase map would be the inverse.
    # This is a placeholder that keeps the original text to avoid losing meaning.
    return chinese


def simple_english_description(name: str, node_type: str, field_type: Optional[str] = None) -> str:
    if node_type == "struct_item":
        return f"`{name}` data structure."
    if node_type == "enum_item":
        return f"`{name}` enumeration."
    if node_type == "trait_item":
        return f"`{name}` trait."
    if node_type == "type_item":
        return f"`{name}` type alias."
    if node_type == "const_item":
        return f"`{name}` constant."
    if node_type == "static_item":
        return f"`{name}` static variable."
    if node_type == "function_item":
        lower = name.lower()
        if lower == "new":
            return "Creates a new instance."
        if lower.startswith("new_"):
            return f"Creates a new `{name[4:]}` instance."
        if lower.startswith("is_"):
            return f"Returns `true` if `{name[3:]}` is true."
        if lower.startswith("get_"):
            return f"Returns the `{name[4:]}` value."
        if lower.startswith("set_"):
            return f"Sets the `{name[4:]}` value."
        if lower.startswith("with_"):
            return f"Returns a copy with `{name[5:]}` set."
        if lower.startswith("from_"):
            return f"Creates `{name[5:]}` from input."
        if lower.startswith("to_"):
            return f"Converts to `{name[3:]}` representation."
        if lower.startswith("parse_"):
            return f"Parses `{name[6:]}` from input."
        if lower.startswith("build_"):
            return f"Builds `{name[6:]}` output."
        return f"`{name}` function."
    if node_type == "enum_variant":
        return f"`{name}` variant."
    if node_type == "field_declaration":
        if field_type:
            return f"`{name}` field of type `{field_type}`."
        return f"`{name}` field."
    if node_type == "mod_item":
        return f"`{name}` module."
    return f"`{name}`."


def build_bilingual_comment(english: str) -> str:
    chinese = translate_en_to_zh(english)
    return f"/// {english}\n/// {chinese}"


def find_doc_comment_block(node: Node, source: bytes) -> tuple[list[str], int]:
    """Return the contiguous outer doc-comment lines directly above a node and the item's start line.

    Scans upward from the declaration line. It stops at a blank line, a non-doc
    `//` comment, or any code line, because those are not part of the item's
    prefix. It continues through attributes (`#[...]`) and outer `///` doc
    comments. The returned start line is the first line of the prefix (the first
    `///` or `#[` line, or the declaration line itself if there is no prefix).
    """
    line = node.start_point[0]
    comments: list[str] = []
    prev_line = line - 1
    while prev_line >= 0:
        start_idx = line_start_byte(prev_line, source)
        end_idx = line_start_byte(prev_line + 1, source)
        line_bytes = source[start_idx:end_idx]
        line_text_str = line_bytes.decode("utf-8", errors="replace")
        stripped = line_text_str.strip()
        if stripped.startswith("///") and not stripped.startswith("////"):
            comments.insert(0, stripped)
            prev_line -= 1
        elif stripped == "" or stripped.startswith("//"):
            # Blank lines and non-doc comments separate this item from the previous one.
            break
        elif stripped.startswith("#["):
            # Attributes belong to the current item; continue scanning for doc comments.
            prev_line -= 1
        else:
            break
    return comments, prev_line + 1


def collect_target_nodes(root: Node, source: bytes) -> list[tuple[Node, str]]:
    targets: list[tuple[Node, str]] = []

    def visit(node: Node, in_impl: bool = False):
        if node.type in (
            "struct_item",
            "enum_item",
            "trait_item",
            "type_item",
            "const_item",
            "static_item",
            "function_item",
            "mod_item",
        ):
            if is_pub(node, source):
                targets.append((node, node.type))
                if node.type == "enum_item":
                    body = first_child_by_type(node, "enum_variant_list")
                    if body:
                        for variant in body.children:
                            if variant.type == "enum_variant":
                                targets.append((variant, "enum_variant"))
                elif node.type == "struct_item":
                    body = first_child_by_type(node, "field_declaration_list")
                    if body:
                        for field in body.children:
                            if field.type == "field_declaration":
                                targets.append((field, "field_declaration"))
                elif node.type == "trait_item":
                    body = first_child_by_type(node, "trait_body")
                    if body:
                        for item in body.children:
                            if item.type in ("function_item", "const_item", "type_item"):
                                targets.append((item, item.type))
            return

        if node.type == "impl_item":
            impl_body = first_child_by_type(node, "declaration_list")
            if impl_body:
                for item in impl_body.children:
                    if item.type == "function_item" and is_pub(item, source):
                        targets.append((item, "function_item"))
            return

        for child in node.children:
            visit(child, in_impl or node.type == "impl_item")

    visit(root)
    return targets


def annotate_file(path: Path, source: bytes, is_core: bool) -> bytes:
    parser = Parser(Language(tree_sitter_rust.language()))
    tree = parser.parse(source)
    root = tree.root_node

    targets = collect_target_nodes(root, source)
    if not targets:
        return source

    # insertions: (line_no, text) to insert before that line
    insertions: list[tuple[int, str]] = []

    for node, node_type in targets:
        name = find_name(node, source)
        if name is None:
            continue

        comments, doc_start_line = find_doc_comment_block(node, source)
        text = parse_doc_text(comments)

        # `doc_start_line` already points to the first line of the item's prefix
        # (the first doc comment or attribute), or the declaration line itself if
        # there is no prefix. Insert new comments there; append after existing ones.
        insert_line = doc_start_line

        # Indentation for the inserted comment should match the item line.
        item_line_text = line_text(node.start_point[0], source)
        indent = len(item_line_text) - len(item_line_text.lstrip())

        # Core files are manually curated; skip them entirely.
        if is_core:
            continue

        # Only generate a bilingual comment for items that have no doc comment.
        # Existing comments (English-only, Chinese-only, or bilingual) are left
        # untouched to avoid clippy doc_lazy_continuation warnings and low-quality
        # auto-translations of long markdown documentation.
        if not comments:
            field_type = None
            if node_type == "field_declaration":
                field_type = find_field_type(node, source)
            english = simple_english_description(name, node_type, field_type)
            comment = (" " * indent + build_bilingual_comment(english).replace("\n", "\n" + " " * indent)) + "\n"
            insertions.append((insert_line, comment))

    if not insertions:
        return source

    # Apply from bottom to top so line numbers stay valid.
    insertions.sort(key=lambda x: x[0], reverse=True)
    lines = source.decode("utf-8").split("\n")

    for line_no, text in insertions:
        lines.insert(line_no, text.rstrip())

    return "\n".join(lines).encode("utf-8")


def main():
    repo_root = Path(__file__).resolve().parent
    for rs_path in repo_root.rglob("*.rs"):
        rel = rs_path.relative_to(repo_root).as_posix()
        is_core = rel in CORE_FILES
        if any(part in rel.split("/") for part in ("tests", "benches", "fuzz", "target", "__pycache__")):
            continue
        source = rs_path.read_bytes()
        new_source = annotate_file(rs_path, source, is_core)
        if new_source != source:
            rs_path.write_bytes(new_source)
            print(f"Annotated: {rel}")


if __name__ == "__main__":
    main()
