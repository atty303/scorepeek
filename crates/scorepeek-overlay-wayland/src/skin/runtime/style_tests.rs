use super::{NativeTree, Node, RenderOutput, attribute_name, html_name};
use blitz_dom::{BaseDocument, DocumentConfig, NodeId};
use scorepeek_skin_sdk::Schedule;
use std::collections::BTreeMap;

fn document() -> (BaseDocument, [NodeId; 2]) {
    let mut doc = BaseDocument::new(DocumentConfig::default());
    let root = doc.root_node().id;
    let mut dom = doc.mutate();
    let html = dom.create_element(html_name("html"), Vec::new());
    let body = dom.create_element(html_name("body"), Vec::new());
    let roots = [
        dom.create_element(html_name("div"), Vec::new()),
        dom.create_element(html_name("div"), Vec::new()),
    ];
    for canvas in roots {
        dom.set_attribute(canvas, attribute_name("class"), "scorepeek-skin-scope");
    }
    dom.append_children(body, &roots);
    dom.append_children(html, &[body]);
    dom.append_children(root, &[html]);
    drop(dom);
    (doc, roots)
}

fn output(id: &str) -> RenderOutput {
    RenderOutput {
        schedule: Schedule::Idle,
        tree: Node::element(
            id,
            "div",
            BTreeMap::from([
                ("id".into(), id.into()),
                ("class".into(), "selection-content".into()),
                ("data-label".into(), ".scorepeek-skin-scope".into()),
            ]),
            Vec::new(),
        ),
    }
}

fn css(width: u32) -> String {
    format!(
        ".scorepeek-skin-scope {{ display:block; width:600px }}
         @media all {{
           .scorepeek-skin-scope :is(.selection-content, [data-unused=',']) {{ display:block; width:{width}px; height:100px }}
           .scorepeek-skin-scope [data-label='.scorepeek-skin-scope'] {{ height:42px }}
         }}"
    )
}

fn size(document: &BaseDocument, id: &str) -> [f64; 2] {
    let node = document.query_selector(&format!("#{id}")).unwrap().unwrap();
    let rect = document.get_client_bounding_rect(node).unwrap();
    [rect.width, rect.height]
}

fn assert_size(document: &BaseDocument, id: &str, expected: [f64; 2]) {
    let actual = size(document, id);
    for (value, expected) in actual.into_iter().zip(expected) {
        assert!((value - expected).abs() < 0.01, "{id}: {actual:?}");
    }
}

#[test]
fn sibling_skin_styles_remain_independent_across_mount_order_and_replacement() {
    for order in [[0, 1], [1, 0]] {
        let (mut document, roots) = document();
        let mut trees: [Option<NativeTree>; 2] = [None, None];
        for i in order {
            let mut tree = NativeTree::new(&mut document, roots[i], &css([512, 22][i])).unwrap();
            tree.apply(&mut document, &output(["left", "right"][i]));
            trees[i] = Some(tree);
        }
        document.resolve(0.0);
        assert_size(&document, "left", [512.0, 42.0]);
        assert_size(&document, "right", [22.0, 42.0]);

        let second = trees[1].as_mut().unwrap();
        second.set_css(&mut document, &css(160)).unwrap();
        document.resolve(0.0);
        assert_size(&document, "left", [512.0, 42.0]);
        assert_size(&document, "right", [160.0, 42.0]);

        second
            .replace(&mut document, &css(96), &output("replacement"))
            .unwrap();
        document.resolve(0.0);
        assert_size(&document, "left", [512.0, 42.0]);
        assert_size(&document, "replacement", [96.0, 42.0]);
        assert!(
            document
                .get_node(roots[1])
                .unwrap()
                .children
                .contains(&second.style)
        );

        second.unmount(&mut document);
        let mut remounted = NativeTree::new(&mut document, roots[1], &css(200)).unwrap();
        remounted.apply(&mut document, &output("remounted"));
        document.resolve(0.0);
        assert_size(&document, "left", [512.0, 42.0]);
        assert_size(&document, "remounted", [200.0, 42.0]);
    }
}
