%% Simple test macro for Dream macro system
-module(test_macro).
-export([simple_derive/1]).

%% A simple derive macro that generates an impl block with a `type_name` method.
%% Input: {struct, 'Name', Fields, TypeParams}
%% Output: {impl, 'Name', [Methods]}
%%
%% Function format:
%%   {function, Name, TypeParams, Params, ReturnType, Body}
%%   where Params = [{Pattern, Type}, ...]
%%   and Body = {Stmts, Expr}
simple_derive({struct, Name, _Fields, _TypeParams}) ->
    %% Generate a method that returns the type name as a string
    Method = {function, 'type_name',
              [],                                      % type params
              [{{ident, 'self'}, {type, any}}],        % params: [{pattern, type}]
              {type, string},                          % return type
              {[], {string, atom_to_binary(Name, utf8)}}  % body: {stmts, expr}
             },
    {impl, Name, [Method]};

simple_derive(Other) ->
    io:format("simple_derive received: ~p~n", [Other]),
    {error, unexpected_input}.
