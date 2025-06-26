use dioxus::prelude::*;

#[derive(Clone, Props, PartialEq)]
pub struct ButtonProps {
    children: Element,

    #[props(default = "default_variant")]
    variant: &'static str,
    
    //#[props(extends = GlobalAttributes)]
    #[props(extends = GlobalAttributes, extends = button)]
    attr: Vec<Attribute>,
}

fn default_variant() -> &'static str {
    "default"
}


#[component]
pub fn Button(props: ButtonProps) -> Element {
    let class = match props.variant {
        "primary" => "px-4 py-2 rounded bg-blue-600 text-white hover:bg-blue-700 text-sm font-medium",
        "outline" => "px-4 py-2 rounded border border-gray-300 hover:bg-gray-100 text-sm font-medium",
        "destructive" => "px-4 py-2 rounded bg-red-600 text-white hover:bg-red-700 text-sm font-medium",
        "disabled" => "px-4 py-2 rounded bg-gray-200 text-gray-500 cursor-not-allowed text-sm font-medium",
        _ => "px-4 py-2 rounded bg-white text-black border border-gray-300 hover:bg-gray-100 text-sm font-medium",
    };

    rsx!(
        button {
            class: class,
            ..props.attr,
            {props.children}
        }
    )
}